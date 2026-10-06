//! XML-RPC, just enough for the forum (mobiquo) backend: build a `methodCall`, read a
//! `methodResponse`. The reader is deliberately lenient — some mobiquo servers emit a struct whose
//! last `</member>` is dropped before `</struct>` — so it keys off opening tags and container ends
//! rather than trusting every close tag.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use quick_xml::Reader;
use quick_xml::events::Event;

/// A parameter we send. Usernames, passwords and search words go as `<base64>` (the plugin's
/// convention), so `Bytes` is a first-class arm beside `Str`.
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

/// A value we read back. `<base64>` decodes to `Str` (lossy UTF-8); numeric ids arrive as `<string>`
/// and stay `Str`, so the backend keeps them as the strings the protocol uses them as.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
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
        match self {
            Self::Int(i) => Some(*i),
            Self::Str(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            Self::Int(i) => Some(*i != 0),
            Self::Str(s) => Some(s == "1" || s.eq_ignore_ascii_case("true")),
            _ => None,
        }
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
    escape_into(&mut s, method);
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
            escape_into(s, v);
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
                escape_into(s, name);
                s.push_str("</name><value>");
                write_arg(s, value);
                s.push_str("</value></member>");
            }
            s.push_str("</struct>");
        }
    }
}

fn escape_into(s: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => s.push_str("&amp;"),
            '<' => s.push_str("&lt;"),
            '>' => s.push_str("&gt;"),
            _ => s.push(c),
        }
    }
}

/// Read the single value of a `methodResponse`, or its fault.
pub fn parse(xml: &[u8]) -> Result<Value, XmlError> {
    let mut reader = Reader::from_reader(xml);
    let cfg = reader.config_mut();
    cfg.check_end_names = false;
    cfg.expand_empty_elements = true;
    let mut p = Parser {
        reader,
        buf: Vec::new(),
        peek: None,
    };
    let mut in_fault = false;
    loop {
        let ev = p.sig()?;
        if ev.is_start(b"fault") {
            in_fault = true;
        } else if ev.is_start(b"value") {
            let value = p.read_value()?;
            return if in_fault {
                Err(fault(&value))
            } else {
                Ok(value)
            };
        } else if ev.is_eof() {
            return Err(XmlError::Empty);
        }
    }
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

/// An event, owned so the reader can move on and so one can be pushed back.
enum Ev {
    Start(Vec<u8>),
    End(Vec<u8>),
    Text(String),
    Eof,
}

impl Ev {
    fn is_start(&self, name: &[u8]) -> bool {
        matches!(self, Ev::Start(n) if n.as_slice() == name)
    }
    fn is_end(&self, name: &[u8]) -> bool {
        matches!(self, Ev::End(n) if n.as_slice() == name)
    }
    fn is_eof(&self) -> bool {
        matches!(self, Ev::Eof)
    }
}

struct Parser<'a> {
    reader: Reader<&'a [u8]>,
    buf: Vec<u8>,
    peek: Option<Ev>,
}

impl Parser<'_> {
    /// The next event, text included; comments and the like are skipped.
    fn raw(&mut self) -> Result<Ev, XmlError> {
        if let Some(ev) = self.peek.take() {
            return Ok(ev);
        }
        loop {
            self.buf.clear();
            let ev = match self.reader.read_event_into(&mut self.buf) {
                Ok(e) => e,
                Err(e) => return Err(XmlError::Parse(e.to_string())),
            };
            return Ok(match ev {
                Event::Start(e) => Ev::Start(e.name().as_ref().to_vec()),
                Event::End(e) => Ev::End(e.name().as_ref().to_vec()),
                Event::Text(t) => Ev::Text(
                    t.unescape()
                        .map(|c| c.into_owned())
                        .unwrap_or_else(|_| String::from_utf8_lossy(t.as_ref()).into_owned()),
                ),
                Event::CData(c) => Ev::Text(String::from_utf8_lossy(c.as_ref()).into_owned()),
                Event::Eof => Ev::Eof,
                _ => continue,
            });
        }
    }

    /// The next event that carries meaning, skipping whitespace between tags.
    fn sig(&mut self) -> Result<Ev, XmlError> {
        loop {
            let ev = self.raw()?;
            if matches!(&ev, Ev::Text(t) if t.trim().is_empty()) {
                continue;
            }
            return Ok(ev);
        }
    }

    fn push(&mut self, ev: Ev) {
        self.peek = Some(ev);
    }

    /// The text inside a scalar element, up to its close.
    fn take_text(&mut self, tag: &[u8]) -> Result<String, XmlError> {
        let mut out = String::new();
        loop {
            match self.raw()? {
                Ev::Text(t) => out.push_str(&t),
                Ev::End(n) if n.as_slice() == tag => break,
                Ev::End(_) | Ev::Eof | Ev::Start(_) => break,
            }
        }
        Ok(out)
    }

    fn skip_end(&mut self, tag: &[u8]) -> Result<(), XmlError> {
        loop {
            match self.raw()? {
                Ev::End(n) if n.as_slice() == tag => return Ok(()),
                Ev::Eof => return Ok(()),
                _ => {}
            }
        }
    }

    /// Positioned just after a `<value>` open.
    fn read_value(&mut self) -> Result<Value, XmlError> {
        let ev = self.sig()?;
        if ev.is_end(b"value") {
            return Ok(Value::Str(String::new()));
        }
        match ev {
            // An untyped `<value>text</value>` is a string.
            Ev::Text(mut s) => {
                loop {
                    match self.raw()? {
                        Ev::Text(x) => s.push_str(&x),
                        Ev::End(n) if n.as_slice() == b"value" => break,
                        other => {
                            self.push(other);
                            break;
                        }
                    }
                }
                Ok(Value::Str(s))
            }
            Ev::Start(name) => {
                let value = self.read_typed(&name)?;
                self.skip_end(b"value")?;
                Ok(value)
            }
            _ => Ok(Value::Str(String::new())),
        }
    }

    fn read_typed(&mut self, name: &[u8]) -> Result<Value, XmlError> {
        if name == b"int" || name == b"i4" || name == b"i8" {
            Ok(Value::Int(
                self.take_text(name)?.trim().parse().unwrap_or(0),
            ))
        } else if name == b"boolean" {
            let t = self.take_text(name)?;
            let t = t.trim();
            Ok(Value::Bool(t == "1" || t.eq_ignore_ascii_case("true")))
        } else if name == b"base64" {
            let raw: String = self.take_text(name)?.split_whitespace().collect();
            let bytes = BASE64.decode(raw).unwrap_or_default();
            Ok(Value::Str(String::from_utf8_lossy(&bytes).into_owned()))
        } else if name == b"array" {
            self.read_array()
        } else if name == b"struct" {
            self.read_struct()
        } else {
            // string, dateTime.iso8601, double, nil or anything unknown: the text as-is.
            Ok(Value::Str(self.take_text(name)?))
        }
    }

    /// Positioned just after an `<array>` open.
    fn read_array(&mut self) -> Result<Value, XmlError> {
        loop {
            let ev = self.sig()?;
            if ev.is_start(b"data") {
                break;
            }
            if ev.is_end(b"array") || ev.is_eof() {
                return Ok(Value::Array(Vec::new()));
            }
        }
        let mut items = Vec::new();
        loop {
            let ev = self.sig()?;
            if ev.is_start(b"value") {
                items.push(self.read_value()?);
            } else if ev.is_end(b"data") {
                break;
            } else if ev.is_end(b"array") || ev.is_eof() {
                return Ok(Value::Array(items));
            }
        }
        self.skip_end(b"array")?;
        Ok(Value::Array(items))
    }

    /// Positioned just after a `<struct>` open. Tolerates a member whose `</member>` was dropped.
    fn read_struct(&mut self) -> Result<Value, XmlError> {
        let mut members = Vec::new();
        loop {
            let ev = self.sig()?;
            if ev.is_start(b"member") {
                members.push(self.read_member()?);
            } else if ev.is_end(b"struct") || ev.is_eof() {
                break;
            }
        }
        Ok(Value::Struct(members))
    }

    fn read_member(&mut self) -> Result<(String, Value), XmlError> {
        let mut key = String::new();
        let mut value = Value::Str(String::new());
        loop {
            let ev = self.sig()?;
            if ev.is_start(b"name") {
                key = self.take_text(b"name")?;
            } else if ev.is_start(b"value") {
                value = self.read_value()?;
            } else if ev.is_end(b"member") {
                break;
            } else if ev.is_start(b"member") || ev.is_end(b"struct") {
                // The close tag was dropped: hand this one back to the struct loop.
                self.push(ev);
                break;
            } else if ev.is_eof() {
                break;
            }
        }
        Ok((key, value))
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
