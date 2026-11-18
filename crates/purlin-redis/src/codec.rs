//! A compact binary form of a [`Snapshot`], so cached and replayed responses survive a
//! round trip through Redis byte for byte.
//!
//! Layout: `status:u16 | header_count:u16 | (name_len:u16 name value_len:u32 value)* | body`.
//! All integers are big-endian. Decoding never panics: anything malformed is `None`,
//! which callers treat as "not there".

use purlin_http::axum::body::Bytes;
use purlin_http::axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use purlin_interceptors::Snapshot;

pub(crate) fn encode(snapshot: &Snapshot) -> Vec<u8> {
    let mut out = Vec::with_capacity(snapshot.body.len() + 128);
    out.extend_from_slice(&snapshot.status.as_u16().to_be_bytes());
    out.extend_from_slice(&(snapshot.headers.len() as u16).to_be_bytes());
    for (name, value) in &snapshot.headers {
        out.extend_from_slice(&(name.as_str().len() as u16).to_be_bytes());
        out.extend_from_slice(name.as_str().as_bytes());
        out.extend_from_slice(&(value.as_bytes().len() as u32).to_be_bytes());
        out.extend_from_slice(value.as_bytes());
    }
    out.extend_from_slice(&snapshot.body);
    out
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        (self.0.len() >= n).then(|| {
            let (head, tail) = self.0.split_at(n);
            self.0 = tail;
            head
        })
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
}

pub(crate) fn decode(bytes: &[u8]) -> Option<Snapshot> {
    let mut reader = Reader(bytes);
    let status = StatusCode::from_u16(reader.u16()?).ok()?;
    let count = reader.u16()?;
    let mut headers = HeaderMap::new();
    for _ in 0..count {
        let name_len = usize::from(reader.u16()?);
        let name = HeaderName::from_bytes(reader.take(name_len)?).ok()?;
        let value_len = reader.u32()? as usize;
        let value = HeaderValue::from_bytes(reader.take(value_len)?).ok()?;
        headers.append(name, value);
    }
    Some(Snapshot {
        status,
        headers,
        body: Bytes::copy_from_slice(reader.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        let mut headers = HeaderMap::new();
        headers.append("content-type", HeaderValue::from_static("application/json"));
        headers.append("set-cookie", HeaderValue::from_static("a=1"));
        headers.append("set-cookie", HeaderValue::from_static("b=2"));
        Snapshot {
            status: StatusCode::CREATED,
            headers,
            body: Bytes::from_static(b"{\"id\":1}\0\xff"),
        }
    }

    #[test]
    fn round_trips_status_repeated_headers_and_binary_bodies() {
        let original = sample();
        let back = decode(&encode(&original)).expect("decodes");
        assert_eq!(back.status, original.status);
        assert_eq!(back.body, original.body);
        assert_eq!(back.headers.get_all("set-cookie").iter().count(), 2);
        assert_eq!(back.headers["content-type"], "application/json");
    }

    #[test]
    fn malformed_input_is_none_never_a_panic() {
        let good = encode(&sample());
        for cut in 0..good.len().min(60) {
            let _ = decode(&good[..cut]); // must not panic
        }
        assert!(decode(&[]).is_none());
        assert!(decode(&[0xff, 0xff, 0, 0]).is_none(), "status 65535 is invalid");
        assert!(decode(&good[..8]).is_none(), "truncated header block");
    }
}
