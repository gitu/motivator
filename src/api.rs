//! OpenAI-compatible chat-completions client (static bearer token).
//! Requests run on background threads; results come back over an mpsc channel.

use std::sync::mpsc::Sender;

use serde_json::{json, Value};

use crate::config::{ApiConfig, Friend};

pub enum ApiEvent {
    /// chat reply from the friend (or error)
    Reply(Result<String, String>),
    /// generated quote lines for a friend
    Generated {
        friend_id: String,
        lines: Result<Vec<String>, String>,
    },
    /// connectivity test result
    Tested(Result<String, String>),
}

pub fn configured(api: &ApiConfig) -> bool {
    !api.base_url.trim().is_empty() && !api.model.trim().is_empty()
}

fn complete(api: &ApiConfig, prompt: &str) -> Result<String, String> {
    let url = format!("{}/chat/completions", api.base_url.trim_end_matches('/'));
    // report http error bodies ourselves instead of ureq's bare status error
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.post(&url).header("Content-Type", "application/json");
    let key = api.api_key.trim();
    if !key.is_empty() {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let mut resp = req
        .send_json(json!({
            "model": api.model,
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0.9,
            "max_tokens": 200,
        }))
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        return Err(format!(
            "http {code}: {}",
            body.chars().take(200).collect::<String>()
        ));
    }
    let body: Value = resp.body_mut().read_json().map_err(|e| e.to_string())?;
    let text = body["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| "malformed response (no choices[0].message.content)".to_string())?;
    let text = text.trim().trim_matches('"').to_string();
    if text.is_empty() {
        Err("empty reply".into())
    } else {
        Ok(text)
    }
}

fn samples(friend: &Friend) -> String {
    friend
        .quotes
        .iter()
        .map(|q| format!("- {}", q.t))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn spawn_reply(
    api: ApiConfig,
    friend: Friend,
    user_text: String,
    tx: Sender<ApiEvent>,
    ctx: egui::Context,
) {
    std::thread::spawn(move || {
        let prompt = format!(
            "You are {}, a friend who motivates me. Your voice, learned from lines you actually say:\n{}\n\nI just told you: \"{}\"\nReply in 1-2 short sentences, lowercase, exactly in that voice. Respond with only the reply text.",
            friend.name,
            samples(&friend),
            user_text
        );
        let _ = tx.send(ApiEvent::Reply(complete(&api, &prompt)));
        ctx.request_repaint();
    });
}

pub fn spawn_generate(
    api: ApiConfig,
    friend: Friend,
    count: usize,
    tx: Sender<ApiEvent>,
    ctx: egui::Context,
) {
    std::thread::spawn(move || {
        let prompt = format!(
            "Lines {} says to motivate a friend:\n{}\nWrite {count} new short lines (max 10 words each) in exactly the same voice and tone. Do not repeat any existing line. Reply with only a JSON array of {count} strings.",
            friend.name,
            samples(&friend)
        );
        let lines = complete(&api, &prompt).and_then(|text| {
            let start = text.find('[').ok_or("no JSON array in reply")?;
            let end = text.rfind(']').ok_or("no JSON array in reply")?;
            let arr: Vec<String> =
                serde_json::from_str(&text[start..=end]).map_err(|e| e.to_string())?;
            Ok(arr.into_iter().take(count).collect())
        });
        let _ = tx.send(ApiEvent::Generated {
            friend_id: friend.id,
            lines,
        });
        ctx.request_repaint();
    });
}

pub fn spawn_test(api: ApiConfig, tx: Sender<ApiEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let result =
            complete(&api, "Reply with the single word: ok").map(|_| "connected ✓".to_string());
        let _ = tx.send(ApiEvent::Tested(result));
        ctx.request_repaint();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// one-shot OpenAI-compatible mock that asserts the bearer token
    fn mock_server(reply_json: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_millis(300)))
                .unwrap();
            let mut raw = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        raw.extend_from_slice(&buf[..n]);
                        let req = String::from_utf8_lossy(&raw);
                        if let Some(head_end) = req.find("\r\n\r\n") {
                            // header names are case-insensitive (ureq 3 lowercases them)
                            let want: usize = req
                                .lines()
                                .find_map(|l| {
                                    l.to_lowercase()
                                        .strip_prefix("content-length: ")
                                        .map(str::to_string)
                                })
                                .and_then(|v| v.trim().parse().ok())
                                .unwrap_or(0);
                            if raw.len() >= head_end + 4 + want {
                                break;
                            }
                        }
                    }
                }
            }
            let req = String::from_utf8_lossy(&raw).to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply_json.len(),
                reply_json
            );
            stream.write_all(resp.as_bytes()).unwrap();
            req
        });
        (format!("http://{addr}/v1"), handle)
    }

    #[test]
    fn complete_parses_reply_and_sends_token() {
        let (base_url, handle) = mock_server(
            r#"{"choices":[{"message":{"role":"assistant","content":"less planning. more shipping."}}]}"#,
        );
        let api = ApiConfig {
            base_url,
            api_key: "sekret-token".into(),
            model: "test-model".into(),
        };
        let out = complete(&api, "hello").unwrap();
        assert_eq!(out, "less planning. more shipping.");
        let req = handle.join().unwrap();
        assert!(req.starts_with("POST /v1/chat/completions"));
        // ureq 3 sends header names lowercased
        assert!(req.contains("authorization: Bearer sekret-token"));
        // don't depend on the client's JSON formatting
        let body = &req[req.find("\r\n\r\n").unwrap() + 4..];
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["model"], "test-model");
    }

    #[test]
    fn complete_reports_malformed_response() {
        let (base_url, _handle) = mock_server(r#"{"unexpected":true}"#);
        let api = ApiConfig {
            base_url,
            api_key: String::new(),
            model: "m".into(),
        };
        let err = complete(&api, "hello").unwrap_err();
        assert!(err.contains("malformed"), "{err}");
    }
}
