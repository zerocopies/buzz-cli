use futures_util::StreamExt;
use reqwest::Response;
use std::error::Error;

enum SseLine {
    Data(String),
    Done,
}

/// Pure line-buffering step: feed it a chunk of bytes, get back the
/// complete SSE data payloads it contains. Kept separate from the async
/// I/O so the actual parsing logic is unit-testable without a live HTTP
/// response (`reqwest::Response` has no public constructor from raw bytes).
fn feed(buf: &mut String, bytes: &[u8]) -> Vec<SseLine> {
    buf.push_str(&String::from_utf8_lossy(bytes));
    let mut out = Vec::new();
    while let Some(pos) = buf.find('\n') {
        let line = buf[..pos].trim_end_matches('\r').to_string();
        buf.drain(..=pos);

        let Some(data) = line
            .strip_prefix("data: ")
            .or_else(|| line.strip_prefix("data:"))
        else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            out.push(SseLine::Done);
        } else if !data.is_empty() {
            out.push(SseLine::Data(data.to_string()));
        }
    }
    out
}

/// Reads a Server-Sent-Events response body, calling `on_data` with each
/// event's payload (the text after `data: `). Stops early if `on_data`
/// returns `false`, or on the OpenAI-style `[DONE]` sentinel (used by
/// Groq); vendors that don't send that sentinel (Gemini) just end the
/// stream naturally when the body closes.
pub async fn for_each_sse_data(
    response: Response,
    mut on_data: impl FnMut(&str) -> bool,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut stream = response.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk?;
        for line in feed(&mut buf, &bytes) {
            match line {
                SseLine::Done => return Ok(()),
                SseLine::Data(data) => {
                    if !on_data(&data) {
                        return Ok(());
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(chunks: &[&str]) -> Vec<String> {
        let mut buf = String::new();
        let mut out = Vec::new();
        for chunk in chunks {
            for line in feed(&mut buf, chunk.as_bytes()) {
                match line {
                    SseLine::Data(d) => out.push(d),
                    SseLine::Done => out.push("__DONE__".to_string()),
                }
            }
        }
        out
    }

    #[test]
    fn extracts_data_payloads() {
        let out = feed_all(&["data: {\"a\":1}\n\ndata: {\"a\":2}\n\n"]);
        assert_eq!(out, vec!["{\"a\":1}".to_string(), "{\"a\":2}".to_string()]);
    }

    #[test]
    fn recognizes_done_sentinel() {
        let out = feed_all(&["data: {\"a\":1}\n\ndata: [DONE]\n\n"]);
        assert_eq!(out, vec!["{\"a\":1}".to_string(), "__DONE__".to_string()]);
    }

    #[test]
    fn handles_a_data_line_split_across_chunks() {
        // The exact scenario bytes_stream() can hand back — a line cut
        // mid-payload across two network reads.
        let out = feed_all(&["data: {\"a\":", "1}\n\n"]);
        assert_eq!(out, vec!["{\"a\":1}".to_string()]);
    }

    #[test]
    fn ignores_non_data_lines_like_event_and_blank() {
        let out = feed_all(&["event: content_block_delta\ndata: {\"a\":1}\n\n"]);
        assert_eq!(out, vec!["{\"a\":1}".to_string()]);
    }

    #[test]
    fn skips_empty_data_payloads() {
        let out = feed_all(&["data: \n\ndata: {\"a\":1}\n\n"]);
        assert_eq!(out, vec!["{\"a\":1}".to_string()]);
    }
}
