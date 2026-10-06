//! DeepSeek（OpenAI 兼容）流式客户端。
//!
//! 配置（base_url / model / API Key）由 `settings` 模块负责读取与持久化，
//! 这里只关心 HTTP 与 SSE。
//!
//! ## 为什么流式里要"切一刀"
//!
//! 对齐表（原文片段 ↔ 译文片段）要跟译文一起要，但**不能让用户看到那坨 JSON**。
//! 所以约定模型在译文之后输出一个分隔符，分隔符之前的内容照常流式推给界面，
//! 之后的内容只悄悄收进 `tail`。
//!
//! 难点是分隔符可能**跨 chunk** 到达，所以不能见到内容就立刻推：
//! 必须扣住末尾 `分隔符长度 - 1` 个字符，等确认它不属于分隔符再推。

use futures_util::StreamExt;
use serde_json::json;
use std::sync::OnceLock;
use std::time::Duration;

pub struct Config {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

pub struct ChatOutcome {
    /// 分隔符之前的正文，也就是译文本身
    pub text: String,
    /// 分隔符之后的内容（对齐 JSON 的原文）；没出现分隔符时为空
    pub tail: String,
}

#[derive(Debug)]
pub struct RequestError {
    pub message: String,
    pub retryable: bool,
}

fn http_client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| format!("构造 HTTP 客户端失败：{error}"))?;
    Ok(CLIENT.get_or_init(|| client))
}

fn request_error(error: reqwest::Error, operation: &str) -> RequestError {
    RequestError {
        retryable: !error.is_builder(),
        message: format!("{operation}：{error}"),
    }
}

fn retryable_status(status: reqwest::StatusCode) -> bool {
    status.as_u16() == 429 || status.as_u16() == 408 || status.is_server_error()
}

fn api_error_message(value: &serde_json::Value) -> Option<String> {
    let error = value.get("error").filter(|error| !error.is_null())?;
    Some(
        error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .or_else(|| error.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| error.to_string()),
    )
}

fn retryable_api_error(value: &serde_json::Value) -> bool {
    let code = value["error"]["code"]
        .as_str()
        .or_else(|| value["error"]["type"].as_str())
        .unwrap_or_default();
    !matches!(
        code,
        "invalid_api_key"
            | "authentication_error"
            | "insufficient_quota"
            | "invalid_request_error"
            | "permission_denied"
            | "content_filter"
    )
}

fn json_request_body(config: &Config, system: &str, user: &str) -> serde_json::Value {
    let mut body = json!({
        "model": config.model,
        "stream": false,
        "temperature": 0,
        "response_format": { "type": "json_object" },
        "max_tokens": 2048,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
    });
    if reqwest::Url::parse(&config.base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .as_deref()
        == Some("api.deepseek.com")
    {
        body["thinking"] = json!({ "type": "disabled" });
    }
    body
}

/// 非流式结构化请求用于词语对齐；调用方控制重试次数和总体时间预算。
pub async fn complete_json(
    config: &Config,
    system: &str,
    user: &str,
) -> Result<String, RequestError> {
    let client = http_client().map_err(|message| RequestError {
        message,
        retryable: false,
    })?;
    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    let response = client
        .post(&url)
        .bearer_auth(&config.api_key)
        .timeout(Duration::from_secs(35))
        .json(&json_request_body(config, system, user))
        .send()
        .await
        .map_err(|error| request_error(error, "词语对齐请求失败"))?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|error| {
        if status.is_success() {
            request_error(error, "读取词语对齐响应失败")
        } else {
            RequestError {
                message: format!("DeepSeek 返回 {status}，读取错误响应失败：{error}"),
                retryable: retryable_status(status),
            }
        }
    })?;
    if !status.is_success() {
        let preview: String = String::from_utf8_lossy(&bytes).chars().take(300).collect();
        return Err(RequestError {
            message: format!("DeepSeek 返回 {status}：{preview}"),
            retryable: retryable_status(status),
        });
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| RequestError {
            message: format!("词语对齐响应 JSON 无效：{error}"),
            retryable: true,
        })?;
    if let Some(message) = api_error_message(&value) {
        return Err(RequestError {
            message: format!("DeepSeek 返回错误：{message}"),
            retryable: retryable_api_error(&value),
        });
    }
    let choice = &value["choices"][0];
    let finish = choice["finish_reason"].as_str().unwrap_or_default();
    if finish != "stop" {
        return Err(RequestError {
            message: format!("词语对齐响应未完整结束（finish_reason={finish:?}）"),
            retryable: !matches!(finish, "content_filter" | "tool_calls" | "function_call"),
        });
    }
    let content = choice["message"]["content"]
        .as_str()
        .filter(|content| !content.trim().is_empty())
        .ok_or_else(|| RequestError {
            message: "词语对齐响应缺少有效内容".to_string(),
            retryable: true,
        })?;
    Ok(content.to_string())
}

pub async fn stream_chat<F>(
    config: &Config,
    system: Option<&str>,
    user: &str,
    on_delta: F,
) -> Result<String, String>
where
    F: FnMut(&str),
{
    stream_chat_split(config, system, user, None, on_delta)
        .await
        .map(|outcome| outcome.text)
}

pub async fn stream_chat_split<F>(
    config: &Config,
    system: Option<&str>,
    user: &str,
    split_marker: Option<&str>,
    mut on_delta: F,
) -> Result<ChatOutcome, String>
where
    F: FnMut(&str),
{
    let client = http_client()?;

    let mut messages = Vec::new();
    if let Some(system) = system {
        messages.push(json!({ "role": "system", "content": system }));
    }
    messages.push(json!({ "role": "user", "content": user }));

    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    let response = client
        .post(&url)
        .bearer_auth(&config.api_key)
        .json(&json!({
            "model": config.model,
            "stream": true,
            "messages": messages,
        }))
        .send()
        .await
        .map_err(|e| format!("请求 DeepSeek 失败：{e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let preview: String = body.chars().take(300).collect();
        return Err(format!("DeepSeek 返回 {status}：{preview}"));
    }

    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::default();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("读取响应流失败：{e}"))?;
        decoder.push(&chunk, split_marker, &mut on_delta)?;
        if decoder.finished {
            break;
        }
    }
    decoder.finish(split_marker, &mut on_delta)
}

#[derive(Default)]
struct SseDecoder {
    buffer: Vec<u8>,
    all: String,
    emitted: usize,
    tail_start: Option<usize>,
    finished: bool,
    stopped: bool,
}

impl SseDecoder {
    fn push<F>(
        &mut self,
        bytes: &[u8],
        split_marker: Option<&str>,
        on_delta: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(&str),
    {
        self.buffer.extend_from_slice(bytes);
        while let Some(index) = self.buffer.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=index).collect();
            self.line(&line[..index], split_marker, on_delta)?;
            if self.finished {
                break;
            }
        }
        Ok(())
    }

    fn line<F>(
        &mut self,
        bytes: &[u8],
        split_marker: Option<&str>,
        on_delta: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(&str),
    {
        let line = std::str::from_utf8(bytes)
            .map_err(|error| format!("DeepSeek 流式响应包含无效 UTF-8：{error}"))?
            .trim_end_matches('\r');
        let Some(data) = line.strip_prefix("data:") else {
            return Ok(());
        };
        let data = data.trim();
        if data.is_empty() {
            return Ok(());
        }
        if data == "[DONE]" {
            self.finished = true;
            return Ok(());
        }
        let value: serde_json::Value = serde_json::from_str(data)
            .map_err(|error| format!("DeepSeek 流式响应 JSON 无效：{error}"))?;
        if let Some(message) = api_error_message(&value) {
            return Err(format!("DeepSeek 返回错误：{message}"));
        }
        let choices = value["choices"]
            .as_array()
            .ok_or_else(|| "DeepSeek 流式响应缺少 choices".to_string())?;
        let Some(choice) = choices.first() else {
            return Ok(()); // stream_options.include_usage 的末尾事件没有 choice。
        };
        if let Some(reason) = choice
            .get("finish_reason")
            .filter(|reason| !reason.is_null())
        {
            match reason.as_str() {
                Some("stop") => self.stopped = true,
                Some("length") => return Err("DeepSeek 流式输出被截断（length）".to_string()),
                Some("content_filter") => {
                    return Err("DeepSeek 流式输出被内容过滤器中止（content_filter）".to_string())
                }
                Some(reason) => return Err(format!("DeepSeek 流式输出异常结束（{reason}）")),
                None => return Err("DeepSeek 流式响应 finish_reason 无效".to_string()),
            }
        }
        if let Some(content) = choice["delta"]
            .get("content")
            .filter(|content| !content.is_null())
        {
            let delta = content
                .as_str()
                .ok_or_else(|| "DeepSeek 流式响应 content 不是字符串".to_string())?;
            self.all.push_str(delta);
            advance(
                &self.all,
                &mut self.emitted,
                &mut self.tail_start,
                split_marker,
                on_delta,
            );
        }
        Ok(())
    }

    fn finish<F>(
        mut self,
        split_marker: Option<&str>,
        on_delta: &mut F,
    ) -> Result<ChatOutcome, String>
    where
        F: FnMut(&str),
    {
        if !self.finished && !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            self.line(&line, split_marker, on_delta)?;
        }
        if !self.finished && !self.stopped {
            return Err("DeepSeek 响应流提前结束，未收到完成标记".to_string());
        }

        let all = self.all;
        let emitted = self.emitted;
        let tail_start = self.tail_start;

        // 流结束了但还有扣住的尾巴没推（例如始终没出现分隔符）
        if tail_start.is_none() && emitted < all.len() {
            on_delta(&all[emitted..]);
        }

        let outcome = match (split_marker, tail_start) {
            (Some(marker), Some(start)) => ChatOutcome {
                text: all[..start - marker.len()].trim_end().to_string(),
                tail: all[start..].to_string(),
            },
            _ => ChatOutcome {
                text: all.trim_end().to_string(),
                tail: String::new(),
            },
        };

        if outcome.text.is_empty() {
            return Err("DeepSeek 返回了空结果".to_string());
        }
        Ok(outcome)
    }
}

/// 决定这次能把 `all` 里的多少安全地推给界面。
fn advance<F>(
    all: &str,
    emitted: &mut usize,
    tail_start: &mut Option<usize>,
    split_marker: Option<&str>,
    on_delta: &mut F,
) where
    F: FnMut(&str),
{
    // 已经在尾段里了：后面全部只收不推
    if tail_start.is_some() {
        return;
    }

    let Some(marker) = split_marker else {
        if all.len() > *emitted {
            on_delta(&all[*emitted..]);
            *emitted = all.len();
        }
        return;
    };

    // 从"还没推出去的位置"开始找分隔符
    if let Some(found) = all[*emitted..].find(marker).map(|i| i + *emitted) {
        if found > *emitted {
            on_delta(&all[*emitted..found]);
        }
        *emitted = found;
        *tail_start = Some(found + marker.len());
        return;
    }

    // 还没见到分隔符：扣住末尾 marker.len() - 1 个字符，
    // 因为那可能是分隔符的前半截。切割点必须落在字符边界上（内容是中文）。
    let holdback = marker.len().saturating_sub(1);
    let mut cut = all.len().saturating_sub(holdback);
    while cut > *emitted && !all.is_char_boundary(cut) {
        cut -= 1;
    }
    if cut > *emitted {
        on_delta(&all[*emitted..cut]);
        *emitted = cut;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc::{self, Receiver};
    use std::thread;

    struct CapturedRequest {
        headers: String,
        body: serde_json::Value,
    }

    /// 所有传输回归只连接本地 HTTP server，不读取用户配置或调用真实 API。
    fn serve(
        status: u16,
        chunks: Vec<Vec<u8>>,
        complete_http: bool,
    ) -> (Config, Receiver<CapturedRequest>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let header_end = loop {
                let mut bytes = [0; 4096];
                let count = socket.read(&mut bytes).unwrap();
                assert!(count > 0, "request closed before headers");
                request.extend_from_slice(&bytes[..count]);
                if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    break index + 4;
                }
            };
            let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
            let length: usize = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .unwrap()
                .1
                .trim()
                .parse()
                .unwrap();
            while request.len() < header_end + length {
                let mut bytes = [0; 4096];
                let count = socket.read(&mut bytes).unwrap();
                assert!(count > 0, "request closed before body");
                request.extend_from_slice(&bytes[..count]);
            }
            sender
                .send(CapturedRequest {
                    headers,
                    body: serde_json::from_slice(&request[header_end..header_end + length])
                        .unwrap(),
                })
                .unwrap();
            write!(
                socket,
                "HTTP/1.1 {status} Test\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            for chunk in chunks {
                write!(socket, "{:x}\r\n", chunk.len()).unwrap();
                socket.write_all(&chunk).unwrap();
                socket.write_all(b"\r\n").unwrap();
                socket.flush().unwrap();
                thread::sleep(Duration::from_millis(5));
            }
            if complete_http {
                // SSE 错误会让客户端提前关闭连接；这不属于测试服务器错误。
                let _ = socket.write_all(b"0\r\n\r\n");
            }
        });
        (
            Config {
                api_key: "local-test-key".to_string(),
                base_url: format!("http://{address}/v1"),
                model: "test-model".to_string(),
            },
            receiver,
            server,
        )
    }

    fn completion(content: &str, finish_reason: &str) -> Vec<u8> {
        json!({
            "choices": [{ "message": { "content": content }, "finish_reason": finish_reason }],
        })
        .to_string()
        .into_bytes()
    }

    /// 把 `advance` 反复作用在一串分片上，模拟真实的增量到达
    fn run(chunks: &[&str], marker: Option<&str>) -> (String, String) {
        let mut all = String::new();
        let mut emitted = 0usize;
        let mut tail_start = None;
        let mut pushed = String::new();

        for chunk in chunks {
            all.push_str(chunk);
            let mut sink = |delta: &str| pushed.push_str(delta);
            advance(&all, &mut emitted, &mut tail_start, marker, &mut sink);
        }
        if tail_start.is_none() && emitted < all.len() {
            pushed.push_str(&all[emitted..]);
        }

        let tail = match (marker, tail_start) {
            (Some(_), Some(start)) => all[start..].to_string(),
            _ => String::new(),
        };
        (pushed, tail)
    }

    #[test]
    fn without_marker_everything_is_pushed() {
        let (pushed, tail) = run(&["你好", "，世界"], None);
        assert_eq!(pushed, "你好，世界");
        assert!(tail.is_empty());
    }

    #[test]
    fn marker_in_one_chunk_splits_correctly() {
        let (pushed, tail) = run(&["译文内容#ALIGN#[{\"a\":1}]"], Some("#ALIGN#"));
        assert_eq!(pushed, "译文内容");
        assert_eq!(tail, "[{\"a\":1}]");
    }

    #[test]
    fn marker_split_across_chunks_is_not_leaked() {
        // 分隔符被切成 "#AL" + "IGN#"：绝不能把 "#AL" 提前推给界面
        let (pushed, tail) = run(&["译文", "#AL", "IGN#", "[]"], Some("#ALIGN#"));
        assert_eq!(pushed, "译文");
        assert_eq!(tail, "[]");
    }

    #[test]
    fn marker_held_back_when_never_completed() {
        // 结尾恰好长得像分隔符前缀，但流断在这里：尾巴仍要推出去，不能吞字
        let (pushed, _tail) = run(&["译文", "#AL"], Some("#ALIGN#"));
        assert_eq!(pushed, "译文#AL");
    }

    #[test]
    fn multibyte_content_is_not_split_mid_character() {
        // 中文是多字节：扣留逻辑必须落在字符边界上，否则会 panic 或产出乱码
        let (pushed, tail) = run(&["这是一段中文译文", "#ALIGN#", "[]"], Some("#ALIGN#"));
        assert_eq!(pushed, "这是一段中文译文");
        assert_eq!(tail, "[]");
    }

    #[tokio::test]
    async fn json_request_uses_structured_deterministic_response_and_keeps_compatible_body() {
        let content = "{\"fragments\":[{\"text\":\"word\",\"before\":\"full \"}]}";
        let (config, capture, server) = serve(200, vec![completion(content, "stop")], true);
        assert_eq!(
            complete_json(&config, "JSON system", "JSON user")
                .await
                .unwrap(),
            content
        );
        let request = capture.recv_timeout(Duration::from_secs(5)).unwrap();
        server.join().unwrap();
        assert!(request
            .headers
            .starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
        assert!(request
            .headers
            .to_ascii_lowercase()
            .contains("authorization: bearer local-test-key"));
        assert_eq!(request.body["stream"], false);
        assert_eq!(request.body["temperature"], 0);
        assert_eq!(request.body["max_tokens"], 2048);
        assert_eq!(request.body["response_format"]["type"], "json_object");
        assert_eq!(request.body["messages"][0]["content"], "JSON system");
        assert_eq!(request.body["messages"][1]["content"], "JSON user");
        assert_eq!(request.body["model"], "test-model");
        assert!(request.body.get("thinking").is_none());
    }

    #[test]
    fn only_official_deepseek_hosts_receive_thinking_configuration() {
        for (base_url, official) in [
            ("https://api.deepseek.com", true),
            ("https://api.deepseek.com/v1/", true),
            ("https://api.deepseek.com.evil.example/v1", false),
            ("https://api.example.com/api.deepseek.com", false),
            ("http://127.0.0.1:1234/v1", false),
        ] {
            let config = Config {
                api_key: "unused".to_string(),
                base_url: base_url.to_string(),
                model: "deepseek-reasoner".to_string(),
            };
            let body = json_request_body(&config, "JSON", "JSON");
            assert_eq!(body.get("thinking").is_some(), official, "{base_url}");
            if official {
                assert_eq!(body["thinking"]["type"], "disabled");
            }
        }
    }

    #[tokio::test]
    async fn json_request_classifies_http_errors_without_hiding_provider_message() {
        for (status, retryable) in [
            (400, false),
            (401, false),
            (402, false),
            (403, false),
            (404, false),
            (408, true),
            (429, true),
            (500, true),
            (503, true),
        ] {
            let (config, capture, server) = serve(
                status,
                vec![br#"{"error":{"message":"local rejection"}}"#.to_vec()],
                true,
            );
            let error = complete_json(&config, "JSON", "JSON").await.unwrap_err();
            assert_eq!(error.retryable, retryable, "HTTP {status}");
            assert!(error.message.contains(&status.to_string()));
            assert!(error.message.contains("local rejection"));
            capture.recv_timeout(Duration::from_secs(5)).unwrap();
            server.join().unwrap();
        }
    }

    #[tokio::test]
    async fn json_request_rejects_truncated_empty_and_http_200_error_results() {
        let cases = [
            (completion("{\"partial\":", "length"), true),
            (completion("{}", "content_filter"), false),
            (completion("{}", "insufficient_system_resource"), true),
            (completion("{}", "aborted"), true),
            (completion(" ", "stop"), true),
            (br#"{"choices":[]}"#.to_vec(), true),
            (br#"{"choices": ["#.to_vec(), true),
            (
                br#"{"error":{"message":"busy","code":"server_error"}}"#.to_vec(),
                true,
            ),
            (
                br#"{"error":{"message":"bad key","code":"invalid_api_key"}}"#.to_vec(),
                false,
            ),
        ];
        for (body, retryable) in cases {
            let (config, capture, server) = serve(200, vec![body], true);
            let error = complete_json(&config, "JSON", "JSON").await.unwrap_err();
            assert_eq!(error.retryable, retryable, "{}", error.message);
            capture.recv_timeout(Duration::from_secs(5)).unwrap();
            server.join().unwrap();
        }
    }

    #[tokio::test]
    async fn json_request_treats_network_disconnect_as_retryable() {
        let (config, capture, server) = serve(200, vec![completion("{}", "stop")], false);
        let error = complete_json(&config, "JSON", "JSON").await.unwrap_err();
        assert!(error.retryable);
        assert!(error.message.contains("读取词语对齐响应失败"));
        capture.recv_timeout(Duration::from_secs(5)).unwrap();
        server.join().unwrap();
    }

    #[tokio::test]
    async fn json_request_keeps_authentication_failure_fatal_when_error_body_disconnects() {
        let (config, capture, server) = serve(401, vec![b"unauthorized".to_vec()], false);
        let error = complete_json(&config, "JSON", "JSON").await.unwrap_err();
        assert!(!error.retryable);
        assert!(error.message.contains("401"));
        capture.recv_timeout(Duration::from_secs(5)).unwrap();
        server.join().unwrap();
    }

    #[tokio::test]
    async fn stream_preserves_utf8_split_between_http_chunks_and_flushes_eof_line() {
        let line = "data: {\"choices\":[{\"delta\":{\"content\":\"模型#ALIGN#[1]\"},\"finish_reason\":\"stop\"}]}";
        let split = line.find('模').unwrap() + 1;
        let (config, capture, server) = serve(
            200,
            vec![
                line.as_bytes()[..split].to_vec(),
                line.as_bytes()[split..].to_vec(),
            ],
            true,
        );
        let mut pushed = String::new();
        let outcome = stream_chat_split(&config, None, "user", Some("#ALIGN#"), |delta| {
            pushed.push_str(delta)
        })
        .await
        .unwrap();
        assert_eq!(pushed, "模型");
        assert_eq!(outcome.text, "模型");
        assert_eq!(outcome.tail, "[1]");
        capture.recv_timeout(Duration::from_secs(5)).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn stream_decoder_preserves_utf8_at_every_possible_transport_boundary() {
        let stream =
            "data: {\"choices\":[{\"delta\":{\"content\":\"你好模型😀\"}}]}\r\n\r\ndata: [DONE]";
        for split in 1..stream.len() {
            let mut decoder = SseDecoder::default();
            let mut pushed = String::new();
            let mut sink = |delta: &str| pushed.push_str(delta);
            decoder
                .push(&stream.as_bytes()[..split], None, &mut sink)
                .unwrap();
            decoder
                .push(&stream.as_bytes()[split..], None, &mut sink)
                .unwrap();
            let result = decoder.finish(None, &mut sink).unwrap();
            assert_eq!(result.text, "你好模型😀", "byte split {split}");
            assert_eq!(pushed, "你好模型😀", "byte split {split}");
        }
    }

    #[tokio::test]
    async fn stream_rejects_invalid_events_and_semantic_truncation() {
        let cases: Vec<Vec<u8>> = vec![
            b"data: not JSON\n\n".to_vec(),
            b"data: \xff\n\n".to_vec(),
            br#"data: {"error":{"message":"stream failed"}}

"#
            .to_vec(),
            br#"data: {"choices":[{"delta":{"content":"partial"}}]}

"#
            .to_vec(),
            br#"data: {"choices":[{"delta":{"content":"partial"},"finish_reason":"length"}]}

"#
            .to_vec(),
            br#"data: {"choices":[{"delta":{},"finish_reason":"content_filter"}]}

"#
            .to_vec(),
            br#"data: {"choices":[{"delta":{},"finish_reason":"insufficient_system_resource"}]}

"#
            .to_vec(),
            br#"data: {"choices":[{"delta":{},"finish_reason":"aborted"}]}

"#
            .to_vec(),
        ];
        for body in cases {
            let (config, capture, server) = serve(200, vec![body], true);
            assert!(stream_chat(&config, None, "user", |_| {}).await.is_err());
            capture.recv_timeout(Duration::from_secs(5)).unwrap();
            server.join().unwrap();
        }
    }
}
