use super::*;

// ── Minimal MCP client (std::net::TcpStream, serde_json) ──────────────────

/// Element/window handle on the MCP wire: `{"index": "N", "generation": "M"}`
/// (pbjson encodes the uint64 values as strings and omits zero fields).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(super) struct McpHandle {
    #[serde(deserialize_with = "pbjson_u64")]
    index: u64,
    #[serde(deserialize_with = "pbjson_u64")]
    generation: u64,
}

impl McpHandle {
    fn to_json(self) -> Value {
        json!({
            "index": self.index.to_string(),
            "generation": self.generation.to_string(),
        })
    }
}

/// A pbjson uint64: a decimal string on the wire, a plain number accepted.
fn pbjson_u64<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Wire {
        Text(String),
        Number(u64),
    }
    match Wire::deserialize(deserializer)? {
        Wire::Text(text) => text.parse().map_err(serde::de::Error::custom),
        Wire::Number(number) => Ok(number),
    }
}

/// Blocking JSON-RPC-over-HTTP client for the embedded MCP server. One
/// connection per call (`Connection: close`) keeps the protocol trivial.
pub(super) struct McpClient {
    addr: SocketAddr,
    next_id: std::cell::Cell<u64>,
}

impl McpClient {
    /// Connect with retries: the server binds asynchronously after the first
    /// window is shown, so the first attempts may be refused.
    pub(super) fn connect(port: u16) -> McpClient {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let mut last_error = String::new();
        for _ in 0..300 {
            if let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(200)) {
                let probe = Self {
                    addr,
                    next_id: std::cell::Cell::new(1),
                }
                .call(&mut stream, "initialize", json!({}));
                match probe {
                    Ok(_) => {
                        return McpClient {
                            addr,
                            next_id: std::cell::Cell::new(2),
                        }
                    }
                    Err(error) => last_error = error,
                }
            } else {
                last_error = "connection refused".into();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        harness_panic(
            "connect to the embedded MCP server",
            &format!("no MCP server answered on {addr}: {last_error}"),
        )
    }

    /// One JSON-RPC call; returns the response's `result` object.
    fn call(&self, stream: &mut TcpStream, method: &str, params: Value) -> HarnessResult<Value> {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let body = rpc_request_body(id, method, params);
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            self.addr,
            body.len(),
            body
        );

        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .and_then(|_| stream.set_write_timeout(Some(Duration::from_secs(30))))
            .map_err(|error| error.to_string())?;
        stream
            .write_all(request.as_bytes())
            .map_err(|error| error.to_string())?;

        let mut response = Vec::new();
        stream
            .take(8 << 20)
            .read_to_end(&mut response)
            .map_err(|error| error.to_string())?;
        let response = String::from_utf8(response)
            .map_err(|error| format!("non-UTF-8 MCP response: {error}"))?;

        let (_, body) = response
            .split_once("\r\n\r\n")
            .ok_or_else(|| format!("malformed HTTP response: {response:.200}"))?;
        rpc_result(body, method)
    }

    fn call_tool(&self, tool: &str, args: Value) -> HarnessResult<Value> {
        let mut stream = TcpStream::connect_timeout(&self.addr, Duration::from_secs(5))
            .map_err(|error| format!("connect for tool {tool}: {error}"))?;
        self.call(&mut stream, "tools/call", tool_call_params(tool, args))
    }

    /// The JSON payload of the first text content block.
    fn call_tool_json(&self, tool: &str, args: Value) -> HarnessResult<Value> {
        tool_text_payload(&self.call_tool(tool, args)?, tool)
    }

    pub(super) fn list_windows(&self) -> HarnessResult<Vec<McpHandle>> {
        let payload = self.call_tool_json("list_windows", json!({}))?;
        required_handles(&payload, "windowHandles")
    }

    pub(super) fn find_elements_by_id(
        &self,
        window: McpHandle,
        id: &str,
    ) -> HarnessResult<Vec<McpHandle>> {
        let payload = self.call_tool_json(
            "find_elements_by_id",
            json!({"windowHandle": window.to_json(), "elementsId": id}),
        )?;
        required_handles(&payload, "elementHandles")
    }

    pub(super) fn query_element_descendants(
        &self,
        element: McpHandle,
        query_stack: Value,
        find_all: bool,
    ) -> HarnessResult<Vec<McpHandle>> {
        let payload = self.call_tool_json(
            "query_element_descendants",
            json!({
                "elementHandle": element.to_json(),
                "queryStack": query_stack,
                "findAll": find_all,
            }),
        )?;
        // A zero-match query serializes to `{}`: the generated proto serde
        // skips empty `elementHandles` arrays. Zero matches is a legitimate
        // outcome for this call (e.g. a ComboBox popup not open yet — the
        // retry loops handle it); `list_windows` / `find_elements_by_id`
        // keep failing loudly on a missing array.
        optional_handles(&payload, "elementHandles")
    }

    pub(super) fn get_element_properties(&self, element: McpHandle) -> HarnessResult<Value> {
        self.call_tool_json(
            "get_element_properties",
            json!({"elementHandle": element.to_json()}),
        )
    }

    /// `action`: omit for a left single click, `"DoubleClick"` for a double
    /// click.
    pub(super) fn click_element(
        &self,
        element: McpHandle,
        action: Option<&str>,
    ) -> HarnessResult<()> {
        let mut args = json!({"elementHandle": element.to_json()});
        if let Some(value) = action {
            args["action"] = json!(value);
        }
        self.call_tool("click_element", args).map(|_| ())
    }

    /// Send a mouse-wheel event over the element's center: a negative
    /// `delta_y` scrolls a Flickable/ScrollView down (revealing content
    /// further down), which is what drives the catalog window's
    /// bottom-edge auto-extension.
    pub(super) fn scroll_element(
        &self,
        element: McpHandle,
        delta_x: f32,
        delta_y: f32,
    ) -> HarnessResult<()> {
        let args = json!({
            "elementHandle": element.to_json(),
            "deltaX": delta_x,
            "deltaY": delta_y,
        });
        self.call_tool("scroll_element", args).map(|_| ())
    }

    pub(super) fn drag_element(
        &self,
        element: McpHandle,
        target_x: f32,
        target_y: f32,
    ) -> HarnessResult<()> {
        let args = json!({
            "elementHandle": element.to_json(),
            "target": {"x": target_x, "y": target_y},
        });
        self.call_tool("drag_element", args).map(|_| ())
    }

    pub(super) fn dispatch_key_event(&self, window: McpHandle, text: &str) -> HarnessResult<()> {
        let args = json!({
            "windowHandle": window.to_json(),
            "text": text,
            "eventType": "PressAndRelease",
        });
        self.call_tool("dispatch_key_event", args).map(|_| ())
    }

    pub(super) fn set_element_value(&self, element: McpHandle, value: &str) -> HarnessResult<()> {
        let args = json!({"elementHandle": element.to_json(), "value": value});
        self.call_tool("set_element_value", args).map(|_| ())
    }

    pub(super) fn get_window_properties(&self, window: McpHandle) -> HarnessResult<Value> {
        self.call_tool_json(
            "get_window_properties",
            json!({"windowHandle": window.to_json()}),
        )
    }

    pub(super) fn take_screenshot(&self, window: McpHandle) -> HarnessResult<Vec<u8>> {
        let result =
            self.call_tool("take_screenshot", json!({"windowHandle": window.to_json()}))?;
        tool_image_png(&result)
    }

    pub(super) fn start_event_recording(&self) -> HarnessResult<Value> {
        self.call_tool_json("start_event_recording", json!({}))
    }

    pub(super) fn stop_event_recording(&self) -> HarnessResult<Value> {
        self.call_tool_json("stop_event_recording", json!({}))
    }
}

// ── MCP wire helpers ──────────────────────────────────────────────────────

/// The JSON-RPC 2.0 request body for one call.
fn rpc_request_body(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

/// The `tools/call` params for `tool`.
fn tool_call_params(tool: &str, args: Value) -> Value {
    json!({"name": tool, "arguments": args})
}

/// The `result` of a JSON-RPC response body. A JSON-RPC `error` and an MCP
/// tool result flagged `isError` are both failures.
fn rpc_result(body: &str, method: &str) -> HarnessResult<Value> {
    let mut response: Value = serde_json::from_str(body).map_err(|error| {
        format!("malformed JSON-RPC response for {method}: {error}: {body:.400}")
    })?;
    if response.get("error").is_some() {
        return Err(format!("JSON-RPC error for {method}: {body:.400}"));
    }
    let result = response
        .get_mut("result")
        .map(Value::take)
        .ok_or_else(|| format!("JSON-RPC response for {method} has no result: {body:.400}"))?;
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(format!("MCP tool error for {method}: {body:.400}"));
    }
    Ok(result)
}

/// The content blocks of an MCP tool result.
fn tool_content(result: &Value) -> &[Value] {
    result
        .get("content")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// Parse the first text content block of a tool result as JSON (the server
/// pretty-prints the proto response into it).
fn tool_text_payload(result: &Value, tool: &str) -> HarnessResult<Value> {
    let text = tool_content(result)
        .iter()
        .find_map(|block| block.get("text").and_then(Value::as_str))
        .ok_or_else(|| format!("no text content block in response for {tool}: {result:.400}"))?;
    serde_json::from_str(text)
        .map_err(|error| format!("malformed {tool} payload: {error}: {text:.400}"))
}

/// Decode the first image content block of a tool result (standard base64).
fn tool_image_png(result: &Value) -> HarnessResult<Vec<u8>> {
    let data = tool_content(result)
        .iter()
        .find_map(|block| block.get("data").and_then(Value::as_str))
        .ok_or_else(|| format!("no image data in screenshot response: {result:.200}"))?;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|error| format!("invalid base64 screenshot data: {error}"))
}

pub(super) fn parse_handle(value: &Value) -> HarnessResult<McpHandle> {
    McpHandle::deserialize(value).map_err(|error| format!("malformed MCP handle {value}: {error}"))
}

/// The handle array under `key`; a missing array is an error.
fn required_handles(payload: &Value, key: &str) -> HarnessResult<Vec<McpHandle>> {
    let array = payload
        .get(key)
        .ok_or_else(|| format!("MCP response missing {key}: {payload:.200}"))?;
    Vec::<McpHandle>::deserialize(array)
        .map_err(|error| format!("malformed {key} in MCP response: {error}: {payload:.200}"))
}

/// The handle array under `key`; a missing array (pbjson omits empty ones)
/// is no handles.
fn optional_handles(payload: &Value, key: &str) -> HarnessResult<Vec<McpHandle>> {
    if payload.get(key).is_some() {
        required_handles(payload, key)
    } else {
        Ok(Vec::new())
    }
}

/// `query_element_descendants` stack: every descendant with accessible
/// role `role`.
pub(super) fn descendants_with_role(role: &str) -> Value {
    json!([{"matchDescendants": true}, {"matchElementAccessibleRole": role}])
}

/// A string-valued element/window property (pbjson omits empty strings).
pub(super) fn str_property<'a>(properties: &'a Value, key: &str) -> Option<&'a str> {
    properties.get(key).and_then(Value::as_str)
}

/// `accessibleEnabled` of an element; pbjson omits it when false.
pub(super) fn accessible_enabled(properties: &Value) -> bool {
    properties.get("accessibleEnabled").and_then(Value::as_bool) == Some(true)
}

/// The `WindowEvent` payloads of one kind (`pointerPressed`, `keyPressed`,
/// …) in a `stop_event_recording` response.
pub(super) fn recorded_events<'a>(
    recording: &'a Value,
    kind: &'a str,
) -> impl Iterator<Item = &'a Value> {
    recording
        .get("events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(move |event| event.get("event")?.get(kind))
}

#[cfg(test)]
mod mcp_wire_tests {
    use super::*;

    /// One-shot fake MCP endpoint: accepts one connection, answers with
    /// `response` and hands back the JSON-RPC body the client sent.
    fn fake_server(response: Value) -> (McpClient, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (sender, receiver) = channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0, "client closed before sending its request");
                request.extend_from_slice(&chunk[..read]);
                let text = String::from_utf8(request.clone()).unwrap();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length: usize = head
                        .lines()
                        .find_map(|line| line.strip_prefix("Content-Length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if body.len() >= length {
                        sender.send(body.to_string()).unwrap();
                        break;
                    }
                }
            }
            let body = response.to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let client = McpClient {
            addr,
            next_id: std::cell::Cell::new(7),
        };
        (client, receiver)
    }

    fn text_response(payload: &str) -> Value {
        json!({"jsonrpc": "2.0", "id": 7, "result": {"content": [{"type": "text", "text": payload}]}})
    }

    /// The request body the previous hand-built client sent for `tool` with
    /// the literal `arguments` text.
    fn hand_built_request(tool: &str, arguments: &str) -> Value {
        serde_json::from_str(&format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/call\",\
             \"params\":{{\"name\":\"{tool}\",\"arguments\":{arguments}}}}}"
        ))
        .unwrap()
    }

    fn sent(receiver: &std::sync::mpsc::Receiver<String>) -> Value {
        serde_json::from_str(&receiver.recv().unwrap()).unwrap()
    }

    const HANDLE: McpHandle = McpHandle {
        index: 3,
        generation: 1,
    };

    #[test]
    fn rpc_envelope_keeps_the_compact_wire_text() {
        assert_eq!(
            rpc_request_body(1, "initialize", json!({})),
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}"
        );
        assert_eq!(
            HANDLE.to_json().to_string(),
            "{\"index\":\"3\",\"generation\":\"1\"}"
        );
    }

    #[test]
    fn tool_requests_round_trip_the_hand_built_shapes() {
        let (client, receiver) =
            fake_server(text_response("{\"elementHandles\": [{\"index\": \"4\"}]}"));
        let found = client
            .find_elements_by_id(HANDLE, "a\"b\\c\n\u{1}")
            .unwrap();
        assert_eq!(
            found,
            vec![McpHandle {
                index: 4,
                generation: 0
            }]
        );
        assert_eq!(
            sent(&receiver),
            hand_built_request(
                "find_elements_by_id",
                r#"{"windowHandle":{"index": "3", "generation": "1"},"elementsId":"a\"b\\c\n\u0001"}"#
            )
        );

        let (client, receiver) = fake_server(text_response("{}"));
        let found = client
            .query_element_descendants(HANDLE, descendants_with_role("Button"), true)
            .unwrap();
        assert!(
            found.is_empty(),
            "pbjson omits an empty elementHandles array"
        );
        assert_eq!(
            sent(&receiver),
            hand_built_request(
                "query_element_descendants",
                r#"{"elementHandle":{"index": "3", "generation": "1"},"queryStack":[{"matchDescendants": true}, {"matchElementAccessibleRole": "Button"}],"findAll":true}"#
            )
        );

        let (client, receiver) = fake_server(text_response("{}"));
        client.click_element(HANDLE, Some("DoubleClick")).unwrap();
        assert_eq!(
            sent(&receiver),
            hand_built_request(
                "click_element",
                r#"{"elementHandle":{"index": "3", "generation": "1"},"action":"DoubleClick"}"#
            )
        );

        let (client, receiver) = fake_server(text_response("{}"));
        client.click_element(HANDLE, None).unwrap();
        assert_eq!(
            sent(&receiver),
            hand_built_request(
                "click_element",
                r#"{"elementHandle":{"index": "3", "generation": "1"}}"#
            )
        );

        let (client, receiver) = fake_server(text_response("{}"));
        client.dispatch_key_event(HANDLE, "\u{1b}").unwrap();
        assert_eq!(
            sent(&receiver),
            hand_built_request(
                "dispatch_key_event",
                r#"{"windowHandle":{"index": "3", "generation": "1"},"text":"\u001b","eventType":"PressAndRelease"}"#
            )
        );

        let (client, receiver) = fake_server(text_response("{}"));
        client.drag_element(HANDLE, 10.5, -3.25).unwrap();
        assert_eq!(
            sent(&receiver),
            hand_built_request(
                "drag_element",
                r#"{"elementHandle":{"index": "3", "generation": "1"},"target":{"x":10.5,"y":-3.25}}"#
            )
        );

        let (client, receiver) = fake_server(text_response("{}"));
        client.scroll_element(HANDLE, 0.5, -50.5).unwrap();
        assert_eq!(
            sent(&receiver),
            hand_built_request(
                "scroll_element",
                r#"{"elementHandle":{"index": "3", "generation": "1"},"deltaX":0.5,"deltaY":-50.5}"#
            )
        );

        let (client, receiver) = fake_server(text_response("{}"));
        client.set_element_value(HANDLE, "Tab\there").unwrap();
        assert_eq!(
            sent(&receiver),
            hand_built_request(
                "set_element_value",
                r#"{"elementHandle":{"index": "3", "generation": "1"},"value":"Tab\there"}"#
            )
        );

        let (client, receiver) = fake_server(text_response("{}"));
        client.stop_event_recording().unwrap();
        assert_eq!(
            sent(&receiver),
            hand_built_request("stop_event_recording", "{}")
        );
    }

    #[test]
    fn responses_parse_escaped_strings_from_the_text_block() {
        // Compact JSON-RPC envelope whose text block carries the server's
        // pretty-printed proto response, escapes included.
        let body = r#"{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"{\n  \"accessibleLabel\": \"Say \\\"hi\\\" \\\\ café\\n\\u001b\",\n  \"accessibleEnabled\": true\n}"}]}}"#;
        let result = rpc_result(body, "tools/call").unwrap();
        let properties = tool_text_payload(&result, "get_element_properties").unwrap();
        assert_eq!(
            str_property(&properties, "accessibleLabel"),
            Some("Say \"hi\" \\ café\n\u{1b}")
        );
        assert!(accessible_enabled(&properties));
        assert_eq!(str_property(&properties, "accessibleValue"), None);
        assert!(!accessible_enabled(&json!({})), "pbjson omits a false flag");
    }

    #[test]
    fn rpc_and_tool_errors_are_failures() {
        let rpc_error =
            r#"{"jsonrpc":"2.0","id":5,"error":{"code":-32601,"message":"Method not found"}}"#;
        let error = rpc_result(rpc_error, "bogus").unwrap_err();
        assert!(error.starts_with("JSON-RPC error for bogus"), "{error}");

        let tool_error = r#"{"jsonrpc":"2.0","id":6,"result":{"content":[{"type":"text","text":"Error: Invalid handle"}],"isError":true}}"#;
        let error = rpc_result(tool_error, "tools/call").unwrap_err();
        assert!(
            error.starts_with("MCP tool error for tools/call"),
            "{error}"
        );

        assert!(tool_text_payload(&json!({"content": []}), "list_windows").is_err());
    }

    #[test]
    fn handle_lists_tolerate_omitted_zero_fields() {
        let payload: Value = serde_json::from_str(
            "{\n  \"windowHandles\": [\n    {\n      \"index\": \"1\",\n      \"generation\": \"2\"\n    },\n    {\n      \"generation\": \"5\"\n    },\n    {}\n  ]\n}",
        )
        .unwrap();
        assert_eq!(
            required_handles(&payload, "windowHandles").unwrap(),
            vec![
                McpHandle {
                    index: 1,
                    generation: 2
                },
                McpHandle {
                    index: 0,
                    generation: 5
                },
                McpHandle {
                    index: 0,
                    generation: 0
                },
            ]
        );
        assert!(required_handles(&payload, "elementHandles").is_err());
        assert!(optional_handles(&payload, "elementHandles")
            .unwrap()
            .is_empty());
        assert!(
            required_handles(&json!({"windowHandles": [{"index": "x"}]}), "windowHandles").is_err()
        );

        let window: Value = serde_json::from_str(
            r#"{"size": {"width": 1150, "height": 625}, "rootElementHandle": {"index": "18446744073709551615", "generation": "3"}}"#,
        )
        .unwrap();
        assert_eq!(
            parse_handle(&window["rootElementHandle"]).unwrap(),
            McpHandle {
                index: u64::MAX,
                generation: 3
            }
        );
    }

    #[test]
    fn recorded_events_select_by_window_event_kind() {
        let recording: Value = serde_json::from_str(
            r#"{"events": [
                {"sequence": "1", "event": {"pointerPressed": {"button": "Left"}}},
                {"sequence": "2", "event": {"pointerReleased": {"button": "Left"}}},
                {"sequence": "3", "event": {"pointerPressed": {"button": "Left"}}},
                {"sequence": "4", "event": {"keyPressed": {"text": "\u001b"}}},
                {"sequence": "5", "event": {"keyReleased": {"text": "\u001b"}}}
            ]}"#,
        )
        .unwrap();
        assert_eq!(recorded_events(&recording, "pointerPressed").count(), 2);
        assert_eq!(recorded_events(&recording, "pointerReleased").count(), 1);
        assert!(recorded_events(&recording, "keyPressed")
            .any(|event| str_property(event, "text") == Some("\u{1b}")));
        assert_eq!(recorded_events(&json!({}), "keyPressed").count(), 0);
    }

    #[test]
    fn screenshot_image_block_decodes_standard_base64() {
        let result = json!({"content": [
            {"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"},
            {"type": "text", "text": "{\n  \"sizeBytes\": 8\n}"}
        ]});
        assert_eq!(
            tool_image_png(&result).unwrap(),
            [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
        );
        assert_eq!(
            tool_image_png(&json!({"content": [{"type": "image", "data": "Zm9vYmFy"}]})).unwrap(),
            b"foobar"
        );
        assert!(tool_image_png(&json!({"content": [{"type": "image", "data": "Zm9v!"}]})).is_err());
        assert!(tool_image_png(&json!({"content": []})).is_err());
    }
}
