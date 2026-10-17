//! Just enough XML and date parsing for the handful of S3 responses we read:
//! `ListObjectsV2` results, `<Error>` documents, `Last-Modified` headers.
//!
//! This is not a general XML parser. S3 responses are machine generated, flat
//! and use no namespaces, attributes or CDATA on the elements read here.

use super::sigv4::days_from_civil;

/// Replaces the five predefined XML entities and numeric character references.
pub(crate) fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else { break };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => out.push(c),
            None => out.push_str(&rest[..=end]),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The inner text of every `<tag>…</tag>` element, in document order.
pub(crate) fn blocks<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let body = &rest[i + open.len()..];
        let Some(j) = body.find(&close) else { break };
        out.push(&body[..j]);
        rest = &body[j + close.len()..];
    }
    out
}

/// The unescaped text of the first `<tag>` element.
pub(crate) fn first_text(xml: &str, tag: &str) -> Option<String> {
    blocks(xml, tag).first().map(|t| unescape(t))
}

/// `2009-10-12T17:50:30.000Z` (also without fraction, or with `+00:00`) → Unix milliseconds.
pub(crate) fn parse_iso8601_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, u32, u32) = (d.next()?.parse().ok()?, d.next()?.parse().ok()?, d.next()?.parse().ok()?);
    let time = time.trim_end_matches('Z').split(['+']).next()?;
    let (hms, frac) = time.split_once('.').unwrap_or((time, ""));
    let mut t = hms.split(':');
    let (h, mi, sec): (i64, i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    let millis: i64 = if frac.is_empty() {
        0
    } else {
        let f: String = frac.chars().take(3).collect();
        format!("{f:0<3}").parse().ok()?
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    Some(((days_from_civil(y, m, day) * 24 + h) * 60 + mi) * 60_000 + sec * 1000 + millis)
}

/// An HTTP date such as `Wed, 21 Oct 2015 07:28:00 GMT` → Unix milliseconds.
pub(crate) fn parse_http_date_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    let s = s.split_once(", ").map_or(s, |(_, rest)| rest);
    let mut parts = s.split_whitespace();
    let day: u32 = parts.next()?.parse().ok()?;
    let name = parts.next()?;
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
        .iter()
        .position(|m| m.eq_ignore_ascii_case(name))? as u32
        + 1;
    let year: i64 = parts.next()?.parse().ok()?;
    let mut t = parts.next()?.split(':');
    let (h, mi, sec): (i64, i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    Some(((days_from_civil(year, month, day) * 24 + h) * 60 + mi) * 60_000 + sec * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_blocks_and_text() {
        let xml = "<R><Contents><Key>a/b</Key><Size>3</Size></Contents><Contents><Key>c&amp;d</Key></Contents><IsTruncated>false</IsTruncated></R>";
        let contents = blocks(xml, "Contents");
        assert_eq!(contents.len(), 2);
        assert_eq!(first_text(contents[0], "Key").as_deref(), Some("a/b"));
        assert_eq!(first_text(contents[1], "Key").as_deref(), Some("c&d"));
        assert_eq!(first_text(xml, "IsTruncated").as_deref(), Some("false"));
        assert_eq!(first_text(xml, "Missing"), None);
        assert!(blocks("<a><b>unterminated", "b").is_empty());
    }

    #[test]
    fn entities_are_unescaped() {
        assert_eq!(unescape("a&lt;b&gt;&amp;&quot;&apos;&#65;&#x42;"), "a<b>&\"'AB");
        assert_eq!(unescape("no entities"), "no entities");
        assert_eq!(unescape("broken & stays"), "broken & stays");
        assert_eq!(unescape("&unknown;x"), "&unknown;x");
    }

    #[test]
    fn dates_parse_to_milliseconds() {
        assert_eq!(parse_iso8601_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_iso8601_ms("2013-05-24T00:00:00Z"), Some(1_369_353_600_000));
        assert_eq!(parse_iso8601_ms("2013-05-24T00:00:00.5Z"), Some(1_369_353_600_500));
        assert_eq!(parse_iso8601_ms("2013-05-24T00:00:00.123456Z"), Some(1_369_353_600_123));
        assert_eq!(parse_iso8601_ms("nonsense"), None);
        assert_eq!(parse_iso8601_ms("2013-13-24T00:00:00Z"), None);
        assert_eq!(parse_http_date_ms("Fri, 24 May 2013 00:00:00 GMT"), Some(1_369_353_600_000));
        assert_eq!(parse_http_date_ms("Wed, 21 Oct 2015 07:28:00 GMT"), Some(1_445_412_480_000));
        assert_eq!(parse_http_date_ms("garbage"), None);
    }
}
