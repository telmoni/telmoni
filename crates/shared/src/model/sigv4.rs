//! AWS Signature Version 4, as Bedrock takes a request: the canonical
//! request hashed, the string to sign, and a key derived from the secret by
//! the day, the region and the service.

use chrono::{DateTime, Utc};
use ring::{digest, hmac};

use crate::digest::hex_lower;

/// `segment` percent-encoded as a URI path segment: every byte but the
/// unreserved ones, upper-case, as SigV4 and every other path here asks.
#[must_use]
pub fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Who signs, where, and when.
#[expect(missing_docs, reason = "each field is what SigV4 names it")]
pub struct Signer<'a> {
    pub access_key_id: &'a str,
    pub secret_access_key: &'a str,
    pub region: &'a str,
    pub service: &'a str,
    pub at: DateTime<Utc>,
}

/// The `Authorization` header for a request: `method` to `host` at `path`,
/// encoded once as it is sent, with `query` already canonical, `headers` the
/// ones signed — lower-case names, `host` and `x-amz-date` among them — and
/// `payload` its body.
#[must_use]
pub fn authorization(
    signer: &Signer<'_>,
    method: &str,
    path: &str,
    query: &str,
    headers: &[(&str, &str)],
    payload: &[u8],
) -> String {
    let mut headers: Vec<(String, String)> = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    headers.sort();
    let canonical_headers: String = headers.iter().map(|(n, v)| format!("{n}:{v}\n")).collect();
    let signed: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();
    let signed = signed.join(";");
    // Every service but S3 encodes each segment of the path again.
    let canonical_uri: String = path
        .split('/')
        .map(encode_segment)
        .collect::<Vec<_>>()
        .join("/");
    let canonical = format!(
        "{method}\n{canonical_uri}\n{query}\n{canonical_headers}\n{signed}\n{}",
        hex_lower(digest::digest(&digest::SHA256, payload).as_ref())
    );
    let day = signer.at.format("%Y%m%d").to_string();
    let stamp = signer.at.format("%Y%m%dT%H%M%SZ").to_string();
    let scope = format!("{day}/{}/{}/aws4_request", signer.region, signer.service);
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{}",
        hex_lower(digest::digest(&digest::SHA256, canonical.as_bytes()).as_ref())
    );
    let mac = |key: &[u8], data: &str| {
        hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data.as_bytes())
    };
    let k_date = mac(format!("AWS4{}", signer.secret_access_key).as_bytes(), &day);
    let k_region = mac(k_date.as_ref(), signer.region);
    let k_service = mac(k_region.as_ref(), signer.service);
    let k_signing = mac(k_service.as_ref(), "aws4_request");
    let signature = hex_lower(mac(k_signing.as_ref(), &to_sign).as_ref());
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={signature}",
        signer.access_key_id
    )
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    /// AWS's own worked example of Signature Version 4, IAM's `ListUsers`,
    /// signs to the signature its documentation gives.
    #[test]
    fn awss_worked_example_signs_as_documented() {
        let signer = Signer {
            access_key_id: "AKIDEXAMPLE",
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            region: "us-east-1",
            service: "iam",
            at: Utc
                .with_ymd_and_hms(2015, 8, 30, 12, 36, 0)
                .single()
                .expect("a time"),
        };
        let header = authorization(
            &signer,
            "GET",
            "/",
            "Action=ListUsers&Version=2010-05-08",
            &[
                (
                    "Content-Type",
                    "application/x-www-form-urlencoded; charset=utf-8",
                ),
                ("Host", "iam.amazonaws.com"),
                ("X-Amz-Date", "20150830T123600Z"),
            ],
            b"",
        );
        assert_eq!(
            header,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/iam/aws4_request, \
             SignedHeaders=content-type;host;x-amz-date, \
             Signature=5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
        );
    }

    /// A segment keeps only the unreserved bytes, and a colon is encoded.
    #[test]
    fn a_segment_keeps_only_the_unreserved() {
        assert_eq!(
            encode_segment("anthropic.claude-3:0"),
            "anthropic.claude-3%3A0"
        );
        assert_eq!(encode_segment("a b/c~"), "a%20b%2Fc~");
    }
}
