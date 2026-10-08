//! 出网包格式：Gist 上真正存放的那份 JSON。
//!
//! 只碰密文与公开头部字段，不认识业务表，也不接触明文配置。
//! 头部刻意保持最小：格式名、格式版本、派生算法与参数、盐、随机数、密文，
//! 外加设备标识、设备名、版本号与时间，方便不解密就能判断冲突。

use serde::{Deserialize, Serialize};

use crate::crypto::KdfParams;
use crate::error::SyncError;

/// 出网包的格式名，写死在包里，用来认领「这是不是一个设置同步包」。
pub const FORMAT_NAME: &str = "ymail-settings-sync";
/// 当前支持的格式版本。
pub const SCHEMA_VERSION: u32 = 1;
/// 派生算法名，目前只认 Argon2id。
pub const KDF_ARGON2ID: &str = "argon2id";

/// 出网包里的派生参数（短名字，省点空间）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvelopeKdfParams {
    /// 内存开销，单位 KiB。
    pub m: u32,
    /// 迭代次数。
    pub t: u32,
    /// 并行度。
    pub p: u32,
}

impl From<KdfParams> for EnvelopeKdfParams {
    fn from(params: KdfParams) -> Self {
        Self {
            m: params.memory_kib,
            t: params.iterations,
            p: params.parallelism,
        }
    }
}

impl From<EnvelopeKdfParams> for KdfParams {
    fn from(params: EnvelopeKdfParams) -> Self {
        Self {
            memory_kib: params.m,
            iterations: params.t,
            parallelism: params.p,
        }
    }
}

/// Gist 上存放的出网包。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncEnvelope {
    /// 格式名，固定 `ymail-settings-sync`。
    pub format: String,
    /// 格式版本。
    pub schema_version: u32,
    /// 派生算法名。
    pub kdf: String,
    /// 派生参数。
    pub kdf_params: EnvelopeKdfParams,
    /// 盐（base64）。
    #[serde(with = "base64_bytes")]
    pub salt: Vec<u8>,
    /// 每次加密新生成的随机数（base64）。
    #[serde(with = "base64_bytes")]
    pub nonce: Vec<u8>,
    /// 密文（base64）。
    #[serde(with = "base64_bytes")]
    pub ciphertext: Vec<u8>,
    /// 本机标识。
    pub device_id: String,
    /// 本机可读名。
    pub device_label: String,
    /// 版本号，上传一次加一。
    pub revision: u64,
    /// 出网时间（RFC 3339 UTC 文本）。
    pub updated_at: String,
}

impl SyncEnvelope {
    /// 解析字节成出网包；格式或版本不认识时给明确错误，不硬着头皮解。
    pub fn parse(bytes: &[u8]) -> Result<Self, SyncError> {
        let envelope: SyncEnvelope = serde_json::from_slice(bytes).map_err(|_| SyncError::Malformed)?;
        envelope.validate()?;
        Ok(envelope)
    }

    /// 序列化成 JSON 字节。
    pub fn to_bytes(&self) -> Result<Vec<u8>, SyncError> {
        serde_json::to_vec(self).map_err(|_| SyncError::Malformed)
    }

    /// 这份包是不是本机写的（用于冲突判定）。
    pub fn authored_by(&self, device_id: &str) -> bool {
        self.device_id == device_id
    }

    /// 校验格式名、格式版本与派生算法。
    fn validate(&self) -> Result<(), SyncError> {
        if self.format != FORMAT_NAME {
            return Err(SyncError::UnsupportedFormat(clean_format_name(&self.format)));
        }
        if self.schema_version != SCHEMA_VERSION {
            return Err(SyncError::UnsupportedFormat(format!(
                "{FORMAT_NAME}/{}",
                self.schema_version
            )));
        }
        if self.kdf != KDF_ARGON2ID {
            return Err(SyncError::UnsupportedFormat(clean_format_name(&self.kdf)));
        }
        Ok(())
    }
}

/// 把格式名清洗成短标识，避免把对方响应里的任意内容塞进错误信息。
fn clean_format_name(raw: &str) -> String {
    raw.chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '/' | '.' | '_'))
        .take(32)
        .collect()
}

/// 二进制字段走 base64 字符串。
mod base64_bytes {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        STANDARD
            .decode(text.as_bytes())
            .map_err(|_| serde::de::Error::custom("base64 解码失败"))
    }
}

#[cfg(test)]
mod tests {
    use super::{EnvelopeKdfParams, SyncEnvelope, FORMAT_NAME, KDF_ARGON2ID, SCHEMA_VERSION};
    use crate::error::SyncError;

    fn sample() -> SyncEnvelope {
        SyncEnvelope {
            format: FORMAT_NAME.to_string(),
            schema_version: SCHEMA_VERSION,
            kdf: KDF_ARGON2ID.to_string(),
            kdf_params: EnvelopeKdfParams { m: 65536, t: 3, p: 1 },
            salt: vec![1, 2, 3, 4],
            nonce: vec![0_u8; 12],
            ciphertext: vec![7, 8, 9],
            device_id: "dev-1".to_string(),
            device_label: "我的电脑".to_string(),
            revision: 12,
            updated_at: "2026-10-08T12:00:00Z".to_string(),
        }
    }

    #[test]
    fn 序列化再解析回来一致() {
        let envelope = sample();
        let bytes = envelope.to_bytes().expect("序列化");
        let back = SyncEnvelope::parse(&bytes).expect("解析");
        assert_eq!(back, envelope);
    }

    #[test]
    fn 二进制字段走base64字符串() {
        let bytes = sample().to_bytes().expect("序列化");
        let text = String::from_utf8(bytes).expect("utf8");
        assert!(text.contains("\"salt\":\"AQIDBA==\""), "盐该走 base64：{text}");
        assert!(text.contains("\"ciphertext\":\"BwgJ\""));
        assert!(!text.contains("[1,2,3,4]"), "不该把字节数组直接塞进 JSON");
    }

    #[test]
    fn 格式名不对直接报错() {
        let mut envelope = sample();
        envelope.format = "something-else".to_string();
        let bytes = serde_json::to_vec(&envelope).expect("序列化");
        assert!(matches!(
            SyncEnvelope::parse(&bytes),
            Err(SyncError::UnsupportedFormat(_))
        ));
    }

    #[test]
    fn 格式版本不认直接报错并带上版本号() {
        let mut envelope = sample();
        envelope.schema_version = 99;
        let bytes = serde_json::to_vec(&envelope).expect("序列化");
        match SyncEnvelope::parse(&bytes) {
            Err(SyncError::UnsupportedFormat(text)) => assert!(text.contains("99")),
            other => panic!("错误类型不对：{other:?}"),
        }
    }

    #[test]
    fn 派生算法不认直接报错() {
        let mut envelope = sample();
        envelope.kdf = "bcrypt".to_string();
        let bytes = serde_json::to_vec(&envelope).expect("序列化");
        assert!(matches!(
            SyncEnvelope::parse(&bytes),
            Err(SyncError::UnsupportedFormat(_))
        ));
    }

    #[test]
    fn 残缺报文报格式错误() {
        assert!(matches!(SyncEnvelope::parse(b"{}"), Err(SyncError::Malformed)));
        assert!(matches!(
            SyncEnvelope::parse(b"not json"),
            Err(SyncError::Malformed)
        ));
    }

    #[test]
    fn 解析时能把base64还原成字节() {
        let json = br#"{"format":"ymail-settings-sync","schemaVersion":1,"kdf":"argon2id","kdfParams":{"m":65536,"t":3,"p":1},"salt":"AQIDBA==","nonce":"AAAAAAAAAAAAAAAA","ciphertext":"BwgJ","deviceId":"dev-1","deviceLabel":"x","revision":1,"updatedAt":"t"}"#;
        let envelope = SyncEnvelope::parse(json).expect("解析");
        assert_eq!(envelope.salt, vec![1, 2, 3, 4]);
        assert_eq!(envelope.nonce, vec![0_u8; 12]);
        assert_eq!(envelope.ciphertext, vec![7, 8, 9]);
    }

    #[test]
    fn 能认出是不是本机写的() {
        let envelope = sample();
        assert!(envelope.authored_by("dev-1"));
        assert!(!envelope.authored_by("dev-2"));
    }
}
