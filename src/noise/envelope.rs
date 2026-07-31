use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    Hs,
    Data,
    Err,
}

impl FrameType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Hs => "hs",
            Self::Data => "data",
            Self::Err => "err",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "hs" => Some(Self::Hs),
            "data" => Some(Self::Data),
            "err" => Some(Self::Err),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub frame_type: FrameType,
    pub payload: Vec<u8>,
}

#[derive(Serialize)]
struct WireFrame<'a> {
    v: i32,
    t: &'a str,
    p: String,
}

#[derive(Deserialize)]
struct WireIn {
    v: i32,
    t: String,
    p: String,
}

pub fn encode_frame(frame: &Frame) -> Result<String, String> {
    let obj = WireFrame {
        v: PROTOCOL_VERSION,
        t: frame.frame_type.as_str(),
        p: to_base64url(&frame.payload),
    };
    serde_json::to_string(&obj).map_err(|e| e.to_string())
}

pub fn decode_frame(raw: &str) -> Result<Frame, String> {
    let obj: WireIn = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    if obj.v != PROTOCOL_VERSION {
        return Err(format!("unsupported version {}", obj.v));
    }
    let frame_type = FrameType::parse(&obj.t).ok_or_else(|| format!("unsupported type {}", obj.t))?;
    let payload = from_base64url(&obj.p)?;
    Ok(Frame { frame_type, payload })
}

fn to_base64url(b: &[u8]) -> String {
    URL_SAFE.encode(b).trim_end_matches('=').to_string()
}

fn from_base64url(s: &str) -> Result<Vec<u8>, String> {
    let pad = (4 - s.len() % 4) % 4;
    let padded = format!("{s}{}", "=".repeat(pad));
    URL_SAFE
        .decode(padded)
        .map_err(|e| e.to_string())
}
