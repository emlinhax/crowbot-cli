//! XML-RPC, just enough for the forum (mobiquo) backend: build a `methodCall`, read a
//! `methodResponse`. The reader is deliberately lenient, since some mobiquo servers drop a close
//! tag (a struct's last `</member>`): it builds a tree that recovers from any dropped one.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use quick_xml::Reader;
use quick_xml::escape::partial_escape;
use quick_xml::events::Event;

/// A parameter we send; `Bytes` goes as `<base64>`.
pub enum Arg {
    Str(String),
    Bytes(Vec<u8>),
    Int(i64),
    Bool(bool),
    // CEILING: no `Array` arm — no call sends one yet; add it (and a `write_arg` case) when one does.
    Struct(Vec<(&'static str, Arg)>),
}

impl Arg {
    pub fn b64(text: &str) -> Self {
        Self::Bytes(text.as_bytes().to_vec())
    }
}

/// A value we read back. Scalars stay their text (`<base64>` decoded, lossy UTF-8) and are read
/// as numbers or flags on request; numeric ids arrive as `<string>`, which is what they are used as.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Array(Vec<Value>),
    Struct(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Struct(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_str().and_then(|s| s.trim().parse().ok())
    }

    pub fn as_bool(&self) -> Option<bool> {
        let s = self.as_str()?.trim();
        Some(s == "1" || s.eq_ignore_ascii_case("true"))
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum XmlError {
    #[error("the forum returned fault {code}: {text}")]
    Fault { code: i64, text: String },
    #[error("could not read the forum's response: {0}")]
    Parse(String),
    #[error("the forum's response held no value")]
    Empty,
}

/// Serialise a call. The body the forum wants: one `<param>` per argument, each wrapped in `<value>`.
pub fn call(method: &str, params: &[Arg]) -> String {
    let mut s =
        String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<methodCall><methodName>");
    s.push_str(&partial_escape(method));
    s.push_str("</methodName><params>");
    for p in params {
        s.push_str("<param><value>");
        write_arg(&mut s, p);
        s.push_str("</value></param>");
    }
    s.push_str("</params></methodCall>");
    s
}

fn write_arg(s: &mut String, arg: &Arg) {
    match arg {
        Arg::Str(v) => {
            s.push_str("<string>");
            s.push_str(&partial_escape(v));
            s.push_str("</string>");
        }
        Arg::Bytes(v) => {
            s.push_str("<base64>");
            s.push_str(&BASE64.encode(v));
            s.push_str("</base64>");
        }
        Arg::Int(v) => s.push_str(&format!("<int>{v}</int>")),
        Arg::Bool(v) => s.push_str(if *v {
            "<boolean>1</boolean>"
        } else {
            "<boolean>0</boolean>"
        }),
        Arg::Struct(members) => {
            s.push_str("<struct>");
            for (name, value) in members {
                s.push_str("<member><name>");
                s.push_str(&partial_escape(*name));
                s.push_str("</name><value>");
                write_arg(s, value);
                s.push_str("</value></member>");
            }
            s.push_str("</struct>");
        }
    }
}

/// Read the single value of a `methodResponse`, or its fault.
pub fn parse(xml: &[u8]) -> Result<Value, XmlError> {
    let root = tree(xml)?;
    if let Some(fault_value) = root.find(b"fault").and_then(|f| f.find(b"value")) {
        return Err(fault(&value(fault_value)));
    }
    root.find(b"value").map(value).ok_or(XmlError::Empty)
}

fn fault(value: &Value) -> XmlError {
    XmlError::Fault {
        code: value.get("faultCode").and_then(Value::as_i64).unwrap_or(0),
        text: value
            .get("faultString")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
    }
}

/// One element of the response: its name, its text, and the elements inside it.
#[derive(Default)]
struct Node {
    name: Vec<u8>,
    text: String,
    children: Vec<Node>,
}

impl Node {
    fn child(&self, name: &[u8]) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    fn all<'a>(&'a self, name: &'a [u8]) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// The first element called `name`, this one or any inside it, depth first.
    fn find(&self, name: &[u8]) -> Option<&Node> {
        if self.name == name {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(name))
    }
}

/// The response as a tree, recovering from dropped close tags: a close tag closes up to the
/// nearest open element of its name (a stray one is ignored), and a start tag matching the open
/// element (a second `<member>` while one is open) closes that first.
fn tree(xml: &[u8]) -> Result<Node, XmlError> {
    let mut reader = Reader::from_reader(xml);
    let cfg = reader.config_mut();
    cfg.check_end_names = false;
    cfg.expand_empty_elements = true;
    let mut open = vec![Node::default()];
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| XmlError::Parse(e.to_string()))?;
        match event {
            Event::Start(e) => {
                let name = e.name().as_ref().to_vec();
                if open.len() > 1 && open.last().is_some_and(|top| top.name == name) {
                    close(&mut open);
                }
                open.push(Node {
                    name,
                    ..Node::default()
                });
            }
            Event::End(e) => {
                let name = e.name();
                if let Some(at) = open.iter().rposition(|n| n.name == name.as_ref())
                    && at > 0
                {
                    while open.len() > at {
                        close(&mut open);
                    }
                }
            }
            Event::Text(t) => {
                let text = t
                    .unescape()
                    .map(|c| c.into_owned())
                    .unwrap_or_else(|_| String::from_utf8_lossy(t.as_ref()).into_owned());
                push_text(&mut open, &text);
            }
            Event::CData(c) => push_text(&mut open, &String::from_utf8_lossy(c.as_ref())),
            Event::Eof => break,
            _ => {}
        }
    }
    while open.len() > 1 {
        close(&mut open);
    }
    Ok(open.pop().unwrap_or_default())
}

fn close(open: &mut Vec<Node>) {
    if let Some(node) = open.pop()
        && let Some(parent) = open.last_mut()
    {
        parent.children.push(node);
    }
}

fn push_text(open: &mut [Node], text: &str) {
    if let Some(top) = open.last_mut() {
        top.text.push_str(text);
    }
}

/// A `<value>` element as a Value: a typed child, or its own text when untyped.
fn value(node: &Node) -> Value {
    let Some(typed) = node.children.first() else {
        let text = if node.text.trim().is_empty() {
            String::new()
        } else {
            node.text.clone()
        };
        return Value::Str(text);
    };
    match typed.name.as_slice() {
        b"base64" => {
            let raw: String = typed.text.split_whitespace().collect();
            let bytes = BASE64.decode(raw).unwrap_or_default();
            Value::Str(String::from_utf8_lossy(&bytes).into_owned())
        }
        b"array" => Value::Array(
            typed
                .child(b"data")
                .map(|data| data.all(b"value").map(value).collect())
                .unwrap_or_default(),
        ),
        b"struct" => Value::Struct(
            typed
                .all(b"member")
                .map(|member| {
                    let name = member.child(b"name").map(|n| n.text.clone());
                    let value = member.child(b"value").map(value);
                    (
                        name.unwrap_or_default(),
                        value.unwrap_or(Value::Str(String::new())),
                    )
                })
                .collect(),
        ),
        // string, int, boolean, double, dateTime.iso8601, nil or anything unknown: the text.
        _ => Value::Str(typed.text.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_wraps_each_argument_in_a_param() {
        let xml = call(
            "login",
            &[Arg::b64("crow"), Arg::b64("pw"), Arg::Bool(false)],
        );
        assert!(xml.contains("<methodName>login</methodName>"));
        // base64("crow") and base64("pw").
        assert!(xml.contains("<base64>Y3Jvdw==</base64>"), "{xml}");
        assert!(xml.contains("<base64>cHc=</base64>"), "{xml}");
        assert!(xml.contains("<boolean>0</boolean>"));
    }

    #[test]
    fn a_struct_of_strings_and_base64_reads_back() {
        // base64("UnKnoWnCheaTs")
        let xml = br#"<?xml version="1.0"?><methodResponse><params><param><value>
          <struct>
            <member><name>sys_version</name><value><string>vb3x_5.0.9</string></value></member>
            <member><name>api_level</name><value><int>3</int></value></member>
            <member><name>guest_okay</name><value><boolean>1</boolean></value></member>
            <member><name>forum_name</name><value><base64>VW5Lbm9XbkNoZWFUcw==</base64></value></member>
          </struct></value></param></params></methodResponse>"#;
        let v = parse(xml).unwrap();
        assert_eq!(v.get_str("sys_version"), Some("vb3x_5.0.9"));
        assert_eq!(v.get("api_level").and_then(Value::as_i64), Some(3));
        assert_eq!(v.get("guest_okay").and_then(Value::as_bool), Some(true));
        assert_eq!(v.get_str("forum_name"), Some("UnKnoWnCheaTs"));
    }

    #[test]
    fn an_array_of_forum_nodes_with_children_reads_back() {
        let xml = br#"<methodResponse><params><param><value><array><data>
          <value><struct>
            <member><name>forum_id</name><value><string>1</string></value></member>
            <member><name>forum_name</name><value><string>Games</string></value></member>
            <member><name>child</name><value><array><data>
              <value><struct>
                <member><name>forum_id</name><value><string>2</string></value></member>
                <member><name>forum_name</name><value><string>CS</string></value></member>
              </struct></value>
            </data></array></value></member>
          </struct></value>
        </data></array></value></param></params></methodResponse>"#;
        let v = parse(xml).unwrap();
        let top = v.as_array().unwrap();
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].get_str("forum_id"), Some("1"));
        let child = top[0].get("child").and_then(Value::as_array).unwrap();
        assert_eq!(child[0].get_str("forum_name"), Some("CS"));
    }

    #[test]
    fn a_dropped_closing_member_still_parses_both_members() {
        // The real server omits the last </member> before </struct>.
        let xml = br#"<methodResponse><params><param><value><struct>
            <member><name>result</name><value><boolean>0</boolean></value></member>
            <member><name>result_text</name><value><base64>bm90IGxvZ2dlZCBpbg==</base64></value>
          </struct></value></param></params></methodResponse>"#;
        let v = parse(xml).unwrap();
        assert_eq!(v.get("result").and_then(Value::as_bool), Some(false));
        assert_eq!(v.get_str("result_text"), Some("not logged in"));
    }

    #[test]
    fn any_dropped_close_tag_is_recovered() {
        // A member whose </member> is missing, then another member, then an array whose </data>
        // is missing.
        let xml = br#"<methodResponse><params><param><value><struct>
            <member><name>a</name><value><string>1</string></value>
            <member><name>b</name><value><array><data><value><string>x</string></value></array></value></member>
          </struct></value></param></params></methodResponse>"#;
        let v = parse(xml).unwrap();
        assert_eq!(v.get_str("a"), Some("1"));
        let b = v.get("b").and_then(Value::as_array).unwrap();
        assert_eq!(b[0].as_str(), Some("x"));
    }

    #[test]
    fn a_fault_becomes_an_error_with_its_code() {
        let xml = br#"<methodResponse><fault><value><struct>
            <member><name>faultCode</name><value><int>3</int></value></member>
            <member><name>faultString</name><value><base64>UmVxdWVzdCBmdW5jdGlvbiBkb2VzIG5vdCBleGlzdA==</base64></value></member>
          </struct></value></fault></methodResponse>"#;
        match parse(xml) {
            Err(XmlError::Fault { code, text }) => {
                assert_eq!(code, 3);
                assert_eq!(text, "Request function does not exist");
            }
            other => panic!("expected a fault, got {other:?}"),
        }
    }
}
