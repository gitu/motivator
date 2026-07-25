//! OpenAI-compatible chat-completions client (static bearer token).
//! Requests run on background threads; results come back over an mpsc channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use serde_json::{json, Value};

use crate::config::{ApiConfig, Friend, TokenParam};

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

/// remembers (per run) that this endpoint wants max_completion_tokens, so
/// only the first request pays the extra probe round trip
static PREFER_COMPLETION_TOKENS: AtomicBool = AtomicBool::new(false);

enum SendErr {
    Transport(String),
    Http(u16, String),
}

fn send(api: &ApiConfig, body: &Value) -> Result<Value, SendErr> {
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
        .send_json(body)
        .map_err(|e| SendErr::Transport(e.to_string()))?;
    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        return Err(SendErr::Http(code, body));
    }
    resp.body_mut()
        .read_json()
        .map_err(|e| SendErr::Transport(e.to_string()))
}

fn complete(api: &ApiConfig, system: &str, user: &str) -> Result<String, String> {
    complete_with_cache(api, system, user, &PREFER_COMPLETION_TOKENS)
}

/// The cap parameter is a moving target: newer OpenAI models 400 on
/// `max_tokens`, older/local servers only know `max_tokens`, and some models
/// also 400 on a non-default temperature. In auto mode we retarget once per
/// rejection instead of failing the request.
fn complete_with_cache(
    api: &ApiConfig,
    system: &str,
    user: &str,
    prefer_completion: &AtomicBool,
) -> Result<String, String> {
    let mut param = match api.token_param {
        TokenParam::MaxTokens => "max_tokens",
        TokenParam::MaxCompletionTokens => "max_completion_tokens",
        TokenParam::Auto => {
            if prefer_completion.load(Ordering::Relaxed) {
                "max_completion_tokens"
            } else {
                "max_tokens"
            }
        }
    };
    let mut messages = Vec::new();
    if !system.trim().is_empty() {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.push(json!({"role": "user", "content": user}));
    let mut temperature = true;
    loop {
        let mut body = json!({
            "model": api.model,
            "messages": messages.clone(),
        });
        if temperature {
            body["temperature"] = json!(0.9);
        }
        body[param] = json!(api.max_tokens);
        match send(api, &body) {
            Ok(reply) => return parse_reply(&reply),
            Err(SendErr::Http(code, err)) => {
                // parameter rejections are always a 400 invalid_request_error;
                // anything else (401, 403, 429, …) is not worth a retry
                if code == 400 {
                    if api.token_param == TokenParam::Auto
                        && param == "max_tokens"
                        && err.contains("max_completion_tokens")
                    {
                        param = "max_completion_tokens";
                        prefer_completion.store(true, Ordering::Relaxed);
                        continue;
                    }
                    if temperature && err.contains("temperature") {
                        temperature = false;
                        continue;
                    }
                }
                return Err(format!(
                    "http {code}: {}",
                    err.chars().take(200).collect::<String>()
                ));
            }
            Err(SendErr::Transport(e)) => return Err(e),
        }
    }
}

fn parse_reply(body: &Value) -> Result<String, String> {
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

/// Ask the endpoint's image-edits API (OpenAI `gpt-image-1`) for the same
/// person with their mouth open — a talking frame for the swap animation.
/// Blocking; returns PNG bytes.
pub fn talk_frame(api: &ApiConfig, photo_png: &[u8]) -> Result<Vec<u8>, String> {
    let url = format!("{}/images/edits", api.base_url.trim_end_matches('/'));
    let boundary = format!("motivator-{:016x}", fastrand::u64(..));
    let mut body = Vec::new();
    let text_part = |body: &mut Vec<u8>, name: &str, value: &str| {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    };
    text_part(&mut body, "model", "gpt-image-1");
    text_part(
        &mut body,
        "prompt",
        "Edit this photo: the exact same person, same framing, same colors, same lighting, \
         but with the mouth clearly open as if speaking mid-sentence. Keep the transparent \
         background fully transparent.",
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; \
             filename=\"photo.png\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(photo_png);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.post(&url).header(
        "Content-Type",
        format!("multipart/form-data; boundary={boundary}"),
    );
    let key = api.api_key.trim();
    if !key.is_empty() {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let mut resp = req.send(&body[..]).map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        return Err(format!(
            "http {code}: {}",
            body.chars().take(200).collect::<String>()
        ));
    }
    let json: Value = resp.body_mut().read_json().map_err(|e| e.to_string())?;
    let b64 = json["data"][0]["b64_json"]
        .as_str()
        .ok_or_else(|| "malformed response (no data[0].b64_json)".to_string())?;
    b64_decode(b64)
}

/// Standard-alphabet base64 (the only shape image APIs return) — small
/// enough that a dependency isn't worth it.
fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    fn val(c: u8) -> Result<u32, String> {
        match c {
            b'A'..=b'Z' => Ok((c - b'A') as u32),
            b'a'..=b'z' => Ok((c - b'a' + 26) as u32),
            b'0'..=b'9' => Ok((c - b'0' + 52) as u32),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(format!("invalid base64 byte {c}")),
        }
    }
    let s: Vec<u8> = s
        .bytes()
        .filter(|b| !matches!(b, b' ' | b'\r' | b'\n' | b'\t'))
        .collect();
    let end = s.iter().position(|&b| b == b'=').unwrap_or(s.len());
    let s = &s[..end];
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for chunk in s.chunks(4) {
        if chunk.len() < 2 {
            return Err("truncated base64".into());
        }
        let mut acc = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            acc |= val(c)? << (18 - 6 * i);
        }
        out.push((acc >> 16) as u8);
        if chunk.len() > 2 {
            out.push((acc >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(acc as u8);
        }
    }
    Ok(out)
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

    /// OpenAI-compatible mock serving one scripted (status, body) per
    /// connection; returns the raw requests it saw, in order
    fn mock_server_seq(
        replies: Vec<(u16, &'static str)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, reply_json) in replies {
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
                requests.push(String::from_utf8_lossy(&raw).to_string());
                let reason = if status == 200 { "OK" } else { "Bad Request" };
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    reply_json.len(),
                    reply_json
                );
                stream.write_all(resp.as_bytes()).unwrap();
            }
            requests
        });
        (format!("http://{addr}/v1"), handle)
    }

    /// one-shot success mock, kept for the simple tests
    fn mock_server(reply_json: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let (url, handle) = mock_server_seq(vec![(200, reply_json)]);
        let handle = std::thread::spawn(move || handle.join().unwrap().remove(0));
        (url, handle)
    }

    /// request body JSON (skips the http header block)
    fn body_of(req: &str) -> Value {
        serde_json::from_str(&req[req.find("\r\n\r\n").unwrap() + 4..]).unwrap()
    }

    const OK_REPLY: &str = r#"{"choices":[{"message":{"role":"assistant","content":"less planning. more shipping."}}]}"#;
    /// verbatim shape of OpenAI's rejection of max_tokens on newer models
    const REJECT_MAX_TOKENS: &str = r#"{"error":{"message":"Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead.","type":"invalid_request_error","param":"max_tokens","code":"unsupported_parameter"}}"#;
    const REJECT_TEMPERATURE: &str = r#"{"error":{"message":"Unsupported value: 'temperature' does not support 0.9 with this model. Only the default (1) value is supported.","type":"invalid_request_error","param":"temperature","code":"unsupported_value"}}"#;

    #[test]
    fn complete_parses_reply_and_sends_token() {
        let (base_url, handle) = mock_server(OK_REPLY);
        let api = ApiConfig {
            base_url,
            api_key: "sekret-token".into(),
            model: "test-model".into(),
            ..Default::default()
        };
        let out = complete(&api, "be brief", "hello").unwrap();
        assert_eq!(out, "less planning. more shipping.");
        let req = handle.join().unwrap();
        assert!(req.starts_with("POST /v1/chat/completions"));
        // ureq 3 sends header names lowercased
        assert!(req.contains("authorization: Bearer sekret-token"));
        // don't depend on the client's JSON formatting
        let body = body_of(&req);
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "be brief");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"], "hello");
        // auto mode leads with the widely-understood param and the configured cap
        assert_eq!(body["max_tokens"], 200);
        assert!(body.get("max_completion_tokens").is_none());
    }

    #[test]
    fn empty_system_prompt_is_omitted() {
        let (base_url, handle) =
            mock_server(r#"{"choices":[{"message":{"role":"assistant","content":"ok"}}]}"#);
        let api = ApiConfig {
            base_url,
            ..Default::default()
        };
        complete(&api, "", "hello").unwrap();
        let req = handle.join().unwrap();
        let messages = body_of(&req)["messages"].as_array().unwrap().clone();
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
            ..Default::default()
        };
        let err = complete(&api, "", "hello").unwrap_err();
        assert!(err.contains("malformed"), "{err}");
    }

    fn friend(persona: &str, chat_prompt: &str, quotes: &[&str]) -> Friend {
        use crate::config::{Accent, Expansion, IdleAnim, PhotoMode, Quote, TalkAnim};
        Friend {
            id: "t".into(),
            name: "marc".into(),
            photo: None,
            photo_mode: PhotoMode::Auto,
            talk_anim: TalkAnim::Jaw,
            idle_anim: IdleAnim::Off,
            blink: true,
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

    #[test]
    fn talk_frame_sends_multipart_and_decodes_b64() {
        // "hello png" == aGVsbG8gcG5n
        let (base_url, handle) = mock_server(r#"{"data":[{"b64_json":"aGVsbG8gcG5n"}]}"#);
        let api = ApiConfig {
            base_url,
            api_key: "tok".into(),
            model: "m".into(),
            ..Default::default()
        };
        let png = b"\x89PNG fake image bytes";
        let out = talk_frame(&api, png).unwrap();
        assert_eq!(out, b"hello png");
        let req = handle.join().unwrap();
        assert!(req.starts_with("POST /v1/images/edits"));
        assert!(req.contains("authorization: Bearer tok"));
        assert!(req.contains("multipart/form-data; boundary=motivator-"));
        assert!(req.contains("name=\"model\"\r\n\r\ngpt-image-1"));
        assert!(req.contains("name=\"prompt\""));
        assert!(req.contains("filename=\"photo.png\""));
        assert!(req.contains("PNG fake image bytes"));
    }

    #[test]
    fn b64_decoder_handles_padding_and_rejects_garbage() {
        assert_eq!(b64_decode("Zg==").unwrap(), b"f");
        assert_eq!(b64_decode("Zm8=").unwrap(), b"fo");
        assert_eq!(b64_decode("Zm9vYmFy").unwrap(), b"foobar");
        assert_eq!(b64_decode("Zm9v\nYmFy").unwrap(), b"foobar", "newlines ok");
        assert!(b64_decode("Zm9v!!").is_err());
    }

    #[test]
    fn falls_back_to_max_completion_tokens() {
        let (base_url, handle) = mock_server_seq(vec![(400, REJECT_MAX_TOKENS), (200, OK_REPLY)]);
        let api = ApiConfig {
            base_url,
            max_tokens: 300,
            ..Default::default()
        };
        let cache = AtomicBool::new(false);
        let out = complete_with_cache(&api, "", "hello", &cache).unwrap();
        assert_eq!(out, "less planning. more shipping.");
        let reqs = handle.join().unwrap();
        let first = body_of(&reqs[0]);
        assert_eq!(first["max_tokens"], 300);
        let second = body_of(&reqs[1]);
        assert_eq!(second["max_completion_tokens"], 300);
        assert!(second.get("max_tokens").is_none());
        // the discovery sticks — the next request skips the probe
        assert!(cache.load(Ordering::Relaxed));
    }

    #[test]
    fn cached_fallback_skips_the_probe() {
        let (base_url, handle) = mock_server_seq(vec![(200, OK_REPLY)]);
        let api = ApiConfig {
            base_url,
            ..Default::default()
        };
        let cache = AtomicBool::new(true);
        complete_with_cache(&api, "", "hello", &cache).unwrap();
        let reqs = handle.join().unwrap();
        let body = body_of(&reqs[0]);
        assert_eq!(body["max_completion_tokens"], 200);
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn explicit_param_mode_does_not_retry() {
        let (base_url, handle) = mock_server_seq(vec![(200, OK_REPLY)]);
        let api = ApiConfig {
            base_url,
            token_param: TokenParam::MaxCompletionTokens,
            ..Default::default()
        };
        let cache = AtomicBool::new(false);
        complete_with_cache(&api, "", "hello", &cache).unwrap();
        let reqs = handle.join().unwrap();
        let body = body_of(&reqs[0]);
        assert_eq!(body["max_completion_tokens"], 200);
        assert!(body.get("max_tokens").is_none());
        // explicit modes never touch the auto-detection cache
        assert!(!cache.load(Ordering::Relaxed));
    }

    #[test]
    fn drops_temperature_when_rejected() {
        let (base_url, handle) = mock_server_seq(vec![(400, REJECT_TEMPERATURE), (200, OK_REPLY)]);
        let api = ApiConfig {
            base_url,
            ..Default::default()
        };
        let cache = AtomicBool::new(false);
        complete_with_cache(&api, "", "hello", &cache).unwrap();
        let reqs = handle.join().unwrap();
        assert!(body_of(&reqs[0]).get("temperature").is_some());
        let second = body_of(&reqs[1]);
        assert!(second.get("temperature").is_none());
        assert_eq!(second["max_tokens"], 200);
    }

    #[test]
    fn only_a_400_triggers_the_fallback() {
        // a 401 body mentioning the magic substring must not cause a retry
        let (base_url, _handle) = mock_server_seq(vec![(401, REJECT_MAX_TOKENS)]);
        let api = ApiConfig {
            base_url,
            ..Default::default()
        };
        let cache = AtomicBool::new(false);
        let err = complete_with_cache(&api, "", "hello", &cache).unwrap_err();
        assert!(err.starts_with("http 401:"), "{err}");
        assert!(!cache.load(Ordering::Relaxed));
    }

    #[test]
    fn unrelated_http_error_is_reported_not_retried() {
        let (base_url, _handle) =
            mock_server_seq(vec![(400, r#"{"error":{"message":"bad model"}}"#)]);
        let api = ApiConfig {
            base_url,
            ..Default::default()
        };
        let cache = AtomicBool::new(false);
        let err = complete_with_cache(&api, "", "hello", &cache).unwrap_err();
        assert!(err.starts_with("http 400:"), "{err}");
        assert!(!cache.load(Ordering::Relaxed));
    }
}
