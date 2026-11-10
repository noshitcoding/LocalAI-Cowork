//! Self-contained stdio MCP entry point; dispatched before Tauri/plugins/AppData.
use serde_json::{json, Value};
use std::io::{BufRead, Read, Write};
use std::time::Duration;

fn search(args: &Value) -> Result<Value, String> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if query.is_empty() || query.len() > 8192 {
        return Err("query must contain 1–8192 bytes".into());
    }
    let default_int = |name: &str, default: u64| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(default)
    };
    let count = args
        .get("maxResults")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| default_int("DDG_MAX_RESULTS", 5))
        .clamp(1, 20) as usize;
    let timeout = args
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| default_int("DDG_TIMEOUT_MS", 10000))
        .clamp(1000, 60000);
    let default_region = std::env::var("DDG_REGION").unwrap_or_else(|_| "wt-wt".into());
    let region = args
        .get("region")
        .and_then(Value::as_str)
        .unwrap_or(&default_region);
    let default_safe = std::env::var("DDG_SAFESEARCH").unwrap_or_else(|_| "moderate".into());
    let safe = args
        .get("safeSearch")
        .and_then(Value::as_str)
        .unwrap_or(&default_safe);
    let kp = match safe {
        "off" => "-2",
        "strict" => "1",
        _ => "-1",
    };
    let endpoint = "https://html.duckduckgo.com/html/";
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(timeout))
        .user_agent("LocalAI-Cowork-WebSearch/1.0")
        .build()
        .map_err(|e| e.to_string())?
        .get(endpoint)
        .query(&[("q", query), ("kl", region), ("kp", kp)])
        .send()
        .map_err(|_| "MCP_SEARCH_UNAVAILABLE: DuckDuckGo request failed".to_owned())?
        .error_for_status()
        .map_err(|e| {
            format!(
                "MCP_SEARCH_HTTP: {}",
                e.status().map(|s| s.as_u16()).unwrap_or(0)
            )
        })?;
    let mut html = String::new();
    response
        .take(2 * 1024 * 1024 + 1)
        .read_to_string(&mut html)
        .map_err(|_| "MCP_SEARCH_INVALID_RESPONSE".to_owned())?;
    if html.len() > 2 * 1024 * 1024 {
        return Err("MCP_SEARCH_RESPONSE_TOO_LARGE".into());
    }
    let results = crate::parse_duckduckgo_results(&html, count);
    let text = if results.is_empty() {
        format!("DuckDuckGo search for: {query}\nNo results found.")
    } else {
        results
            .iter()
            .enumerate()
            .map(|(i, result)| {
                format!(
                    "{}. {}\n{}\n{}",
                    i + 1,
                    result.title,
                    result.url,
                    result.snippet
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    Ok(
        json!({"content":[{"type":"text","text":text}],"structuredContent":{"query":query,"region":region,"safeSearch":safe,"count":results.len(),"results":results}}),
    )
}

fn handle(
    request: Value,
    search_fn: impl FnOnce(&Value) -> Result<Value, String>,
) -> Option<Value> {
    let id = request.get("id")?.clone();
    let result = match request.get("method").and_then(Value::as_str).unwrap_or("") {
        "initialize" => Ok(
            json!({"protocolVersion":"2024-11-05","serverInfo":{"name":"localai-cowork-duckduckgo-websearch","version":"1.0.0"},"capabilities":{"tools":{}}}),
        ),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(
            json!({"tools":[{"name":"search_web","description":"Search the web with DuckDuckGo and return top results.","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"maxResults":{"type":"integer","minimum":1,"maximum":20},"region":{"type":"string"},"safeSearch":{"type":"string","enum":["off","moderate","strict"]},"timeoutMs":{"type":"integer","minimum":1000,"maximum":60000}},"required":["query"]}}]}),
        ),
        "tools/call" if request["params"]["name"] == "search_web" => {
            search_fn(&request["params"]["arguments"])
        }
        _ => {
            return Some(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method or tool not found"}}),
            )
        }
    };
    Some(match result {
        Ok(value) => json!({"jsonrpc":"2.0","id":id,"result":value}),
        Err(error) => {
            json!({"jsonrpc":"2.0","id":id,"result":{"isError":true,"content":[{"type":"text","text":error}]}})
        }
    })
}

pub fn dispatch() -> Option<i32> {
    if !std::env::args().any(|arg| arg == "--lacowork-mcp-websearch") {
        return None;
    }
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut stdout = std::io::stdout().lock();
    loop {
        let mut line = String::new();
        match (&mut reader).take(1024 * 1024 + 1).read_line(&mut line) {
            Ok(0) => return Some(0),
            Err(_) => return Some(1),
            _ if line.len() > 1024 * 1024 => return Some(1),
            _ => {}
        }
        let response = match serde_json::from_str(&line) {
            Ok(request) => handle(request, search),
            Err(_) => Some(
                json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}),
            ),
        };
        if let Some(response) = response {
            if writeln!(stdout, "{response}")
                .and_then(|_| stdout.flush())
                .is_err()
            {
                return Some(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn initialize_and_list_work_without_network_or_gui() {
        let never_search =
            |_: &Value| -> Result<Value, String> { panic!("network called during discovery") };
        assert_eq!(
            handle(json!({"id":1,"method":"initialize"}), never_search).unwrap()["result"]
                ["protocolVersion"],
            "2024-11-05"
        );
        assert_eq!(
            handle(json!({"id":2,"method":"tools/list"}), never_search).unwrap()["result"]["tools"]
                [0]["name"],
            "search_web"
        );
        assert!(handle(json!({"method":"notifications/initialized"}), never_search).is_none());
    }
    #[test]
    fn search_dispatch_uses_bounded_adapter_and_preserves_errors() {
        let request = json!({"id":3,"method":"tools/call","params":{"name":"search_web","arguments":{"query":"fixture"}}});
        assert_eq!(
            handle(request.clone(), |args| {
                assert_eq!(args["query"], "fixture");
                Ok(json!({"content":[]}))
            })
            .unwrap()["result"]["content"],
            json!([])
        );
        assert_eq!(
            handle(request, |_| Err("offline".into())).unwrap()["result"]["isError"],
            true
        );
    }
}
