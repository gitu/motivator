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

fn complete(api: &ApiConfig, system: &str, user: &str) -> Result<String, String> {
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
    let mut messages = Vec::new();
    if !system.trim().is_empty() {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": user}));
    let mut resp = req
        .send_json(json!({
            "model": api.model,
            "messages": messages,
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

/// The chat system prompt: the friend's custom prompt if set (with {name},
/// {description} and {quotes} placeholders substituted), otherwise the
/// built-in template from name + description + sample quotes.
pub fn chat_system_prompt(friend: &Friend) -> String {
    let custom = friend.chat_prompt.trim();
    if !custom.is_empty() {
        return custom
            .replace("{name}", &friend.name)
            .replace("{description}", friend.persona.trim())
            .replace("{quotes}", &samples(friend));
    }
    let mut p = format!("You are {}, a friend who motivates me.", friend.name);
    let persona = friend.persona.trim();
    if !persona.is_empty() {
        p.push_str(&format!(" {persona}"));
    }
    if !friend.quotes.is_empty() {
        p.push_str(&format!(
            "\nYour voice, learned from lines you actually say:\n{}",
            samples(friend)
        ));
    }
    p.push_str("\nReply in 1-2 short sentences, lowercase, exactly in that voice. Respond with only the reply text.");
    p
}

/// The quote-generation prompt: persona (when set) plus existing lines as
/// voice anchor and do-not-repeat list — works with either one alone.
fn generate_prompt(friend: &Friend, count: usize) -> String {
    let mut p = format!(
        "{} motivates a friend with short punchy lines.",
        friend.name
    );
    let persona = friend.persona.trim();
    if !persona.is_empty() {
        p.push_str(&format!(" {persona}"));
    }
    if !friend.quotes.is_empty() {
        p.push_str(&format!(
            "\nLines they already say:\n{}\nWrite {count} new short lines (max 10 words each) in exactly the same voice and tone. Do not repeat any existing line.",
            samples(friend)
        ));
    } else {
        p.push_str(&format!(
            "\nWrite {count} short lines (max 10 words each) they would say, in that voice."
        ));
    }
    p.push_str(&format!(
        " Reply with only a JSON array of {count} strings."
    ));
    p
}

pub fn spawn_reply(
    api: ApiConfig,
    friend: Friend,
    user_text: String,
    tx: Sender<ApiEvent>,
    ctx: egui::Context,
) {
    std::thread::spawn(move || {
        let system = chat_system_prompt(&friend);
        let user = format!("I just told you: \"{user_text}\"");
        let _ = tx.send(ApiEvent::Reply(complete(&api, &system, &user)));
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
        let prompt = generate_prompt(&friend, count);
        let lines = complete(&api, "", &prompt).and_then(|text| {
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
            complete(&api, "", "Reply with the single word: ok").map(|_| "connected ✓".to_string());
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
        let out = complete(&api, "be brief", "hello").unwrap();
        assert_eq!(out, "less planning. more shipping.");
        let req = handle.join().unwrap();
        assert!(req.starts_with("POST /v1/chat/completions"));
        // ureq 3 sends header names lowercased
        assert!(req.contains("authorization: Bearer sekret-token"));
        // don't depend on the client's JSON formatting
        let body = &req[req.find("\r\n\r\n").unwrap() + 4..];
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "be brief");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"], "hello");
    }

    #[test]
    fn empty_system_prompt_is_omitted() {
        let (base_url, handle) =
            mock_server(r#"{"choices":[{"message":{"role":"assistant","content":"ok"}}]}"#);
        let api = ApiConfig {
            base_url,
            api_key: String::new(),
            model: "m".into(),
        };
        complete(&api, "", "hello").unwrap();
        let req = handle.join().unwrap();
        let body = &req[req.find("\r\n\r\n").unwrap() + 4..];
        let body: Value = serde_json::from_str(body).unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn complete_reports_malformed_response() {
        let (base_url, _handle) = mock_server(r#"{"unexpected":true}"#);
        let api = ApiConfig {
            base_url,
            api_key: String::new(),
            model: "m".into(),
        };
        let err = complete(&api, "", "hello").unwrap_err();
        assert!(err.contains("malformed"), "{err}");
    }

    fn friend(persona: &str, chat_prompt: &str, quotes: &[&str]) -> Friend {
        use crate::config::{Accent, Expansion, Quote};
        Friend {
            id: "t".into(),
            name: "marc".into(),
            photo: None,
            split: 0.52,
            persona: persona.into(),
            chat_prompt: chat_prompt.into(),
            accent: Accent::Orange,
            quotes: quotes.iter().map(|t| Quote::sample(t)).collect(),
            pool: Vec::new(),
            expansion: Expansion::Off,
            nudges: false,
            interval_secs: 60,
        }
    }

    #[test]
    fn default_chat_prompt_combines_name_persona_and_quotes() {
        let p = chat_system_prompt(&friend("blunt. direct.", "", &["do it now"]));
        assert!(p.contains("You are marc"), "{p}");
        assert!(p.contains("blunt. direct."), "{p}");
        assert!(p.contains("- do it now"), "{p}");
        assert!(p.contains("1-2 short sentences"), "{p}");
    }

    #[test]
    fn default_chat_prompt_works_without_persona_or_quotes() {
        let p = chat_system_prompt(&friend("", "", &[]));
        assert!(p.contains("You are marc"), "{p}");
        assert!(!p.contains("lines you actually say"), "{p}");
    }

    #[test]
    fn custom_chat_prompt_substitutes_placeholders() {
        let f = friend("grumpy", "Play {name} ({description}):\n{quotes}", &["go"]);
        assert_eq!(chat_system_prompt(&f), "Play marc (grumpy):\n- go");
        // plain override without placeholders is used verbatim
        let f = friend("grumpy", "just be nice", &["go"]);
        assert_eq!(chat_system_prompt(&f), "just be nice");
    }

    #[test]
    fn generate_prompt_uses_persona_and_quotes() {
        let p = generate_prompt(&friend("stoic calm", "", &["breathe"]), 5);
        assert!(p.contains("stoic calm"), "{p}");
        assert!(p.contains("- breathe"), "{p}");
        assert!(p.contains("Do not repeat"), "{p}");
        assert!(p.contains("JSON array of 5 strings"), "{p}");
        // description alone is enough — no quotes required
        let p = generate_prompt(&friend("stoic calm", "", &[]), 3);
        assert!(p.contains("stoic calm"), "{p}");
        assert!(p.contains("JSON array of 3 strings"), "{p}");
        assert!(!p.contains("already say"), "{p}");
    }
}
