//! Timestamp utility functions

/// Get the current Unix timestamp in seconds, returns 0 on failure.
///
/// Replaces scattered `SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()` calls.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Get the current Unix timestamp in milliseconds, returns 0 on failure.
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── 本地时区时间（面向用户展示用）──────────────────────────────────────────
//
// 项目时间策略：存储内部一律用 UTC（绝对时刻，无歧义），仅在"输出给人看"
// 的场景（API 序列化、日志、SQLite 默认值）转成本地时区。下面这些函数
// 是该策略的统一入口，避免各处散落地调 chrono::Local::now()。
// epoch 整数路径（now_secs/now_millis）与时区无关，不在此列。

/// 当前本地时区时间（带偏移），用于面向用户的时间展示。
///
/// 序列化为带偏移 RFC3339（如 `2026-07-09T09:50:48.876+08:00`）。
/// 注意：存储内部仍用 UTC（绝对时刻安全），本函数仅用于"输出给人看"的场景。
/// 读取系统时区（`TZ` 环境变量或系统配置）。
pub fn now_local() -> chrono::DateTime<chrono::FixedOffset> {
    chrono::Local::now().fixed_offset()
}

/// 把任意时区的 `DateTime` 转成本地偏移（序列化/展示用）。
///
/// 典型用法：把内部存储的 `DateTime<Utc>` 转成本地偏移后再 `to_rfc3339()`。
pub fn to_local<Tz: chrono::TimeZone>(
    dt: chrono::DateTime<Tz>,
) -> chrono::DateTime<chrono::FixedOffset> {
    dt.with_timezone(&chrono::Local).fixed_offset()
}

/// serde 模块：把 `DateTime<Utc>` 序列化成带本地偏移的 RFC3339。
///
/// 用法：`#[serde(with = "echo_core::utils::time::local_rfc3339")]`
///
/// - 序列化方向：`DateTime<Utc>` → 本地偏移 RFC3339 字符串（前端 `new Date()` 可正确解析）。
/// - 反序列化方向：标准 RFC3339 解析回 `DateTime<Utc>`，对 UTC(`Z`) 和本地偏移(`+08:00`)
///   字符串都兼容，历史数据无需迁移。
///
/// 字段类型保持 `DateTime<Utc>` 不变（方案 B：不动类型，只改序列化输出）。
pub mod local_rfc3339 {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(dt: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::to_local(*dt).to_rfc3339())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime<Utc>, D::Error> {
        // 标准解析：带 Z 或 +08:00 的 RFC3339 都能正确读回并归一到 UTC。
        DateTime::parse_from_rfc3339(&String::deserialize(d)?)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(serde::de::Error::custom)
    }
}

/// serde 模块：`Option<DateTime<Utc>>` 的本地偏移 RFC3339 序列化。
///
/// 用法：`#[serde(with = "echo_core::utils::time::option_local_rfc3339")]`
///
/// `None` 序列化为 JSON `null`；`Some` 走与 [`local_rfc3339`] 相同的本地偏移规则。
pub mod option_local_rfc3339 {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(opt: &Option<DateTime<Utc>>, s: S) -> Result<S::Ok, S::Error> {
        match opt {
            Some(dt) => s.serialize_some(&super::to_local(*dt).to_rfc3339()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<DateTime<Utc>>, D::Error> {
        let opt = Option::<String>::deserialize(d)?;
        match opt {
            None => Ok(None),
            Some(s) => DateTime::parse_from_rfc3339(&s)
                .map(|dt| Some(dt.with_timezone(&Utc)))
                .map_err(serde::de::Error::custom),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_now_secs_reasonable() {
        let ts = now_secs();
        assert!(ts > 1_704_067_200, "timestamp should be after 2024");
        assert!(ts < 4_102_444_800, "timestamp should be before 2100");
    }

    #[test]
    fn test_now_millis_monotonic() {
        let a = now_millis();
        let b = now_millis();
        assert!(b >= a);
    }

    #[test]
    fn now_local_matches_utc_instant() {
        // CI 可能跑在 UTC（偏移 0），只断言与 Utc::now 表示同一绝对时刻。
        let now = now_local();
        let utc = chrono::Utc::now();
        let diff = (now.timestamp() - utc.timestamp()).abs();
        assert!(diff <= 1, "now_local 与 Utc::now 应表示同一时刻");
    }

    #[test]
    fn to_local_preserves_instant() {
        let utc = chrono::Utc::now();
        let local = to_local(utc);
        assert_eq!(utc.timestamp(), local.timestamp());
    }

    #[test]
    fn local_rfc3339_roundtrip() {
        use chrono::{DateTime, Utc};
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            #[serde(with = "local_rfc3339")]
            ts: DateTime<Utc>,
        }

        let original = Wrapper {
            ts: match "2026-07-09T01:50:48.876Z".parse::<DateTime<Utc>>() {
                Ok(dt) => dt,
                Err(e) => panic!("fixture parse failed: {e}"),
            },
        };
        let json = match serde_json::to_string(&original) {
            Ok(s) => s,
            Err(e) => panic!("serialize failed: {e}"),
        };
        // 序列化输出应带本地偏移（+ 或 -），而非仅 UTC 的 Z。
        // 注意：RFC3339 日期部分也含 `-`（如 2026-07-09），所以用更精确的偏移检测。
        let has_offset = json.contains("+")
            || json.ends_with("-00:00\"}")
            || json.contains("T")
                && (json.contains("+") || {
                    // 找时间部分后的偏移：...HH:MM:SS[.fff]±HH:MM
                    json.rfind('T')
                        .and_then(|i| {
                            json.get(i..)
                                .and_then(|suffix| suffix.find(['+', '-']).map(|j| i + j))
                        })
                        .is_some_and(|off| {
                            json.get(off..)
                                .and_then(|suffix| suffix.chars().nth(1))
                                .is_some_and(|c| c.is_ascii_digit())
                        })
                });
        assert!(has_offset, "序列化应输出带偏移 RFC3339,实际: {json}");
        // 明确不应以 Z 结尾（本地偏移格式）
        assert!(
            !json.contains("Z\""),
            "序列化不应输出 UTC Z 后缀,实际: {json}"
        );

        let parsed: Wrapper = match serde_json::from_str(&json) {
            Ok(v) => v,
            Err(e) => panic!("deserialize failed: {e}"),
        };
        assert_eq!(parsed.ts, original.ts);
    }

    #[test]
    fn local_rfc3339_deserialize_legacy_utc() {
        use chrono::{DateTime, Utc};
        use serde::Deserialize;

        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(with = "local_rfc3339")]
            ts: DateTime<Utc>,
        }

        let json = r#"{"ts":"2026-07-09T01:50:48.876Z"}"#;
        let parsed: Wrapper = match serde_json::from_str(json) {
            Ok(v) => v,
            Err(e) => panic!("deserialize failed: {e}"),
        };
        assert_eq!(parsed.ts.timestamp(), 1_783_561_848);
    }

    #[test]
    fn option_local_rfc3339_none_and_some() {
        use chrono::{DateTime, Utc};
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Wrapper {
            #[serde(with = "option_local_rfc3339")]
            ts: Option<DateTime<Utc>>,
        }

        let none = Wrapper { ts: None };
        let none_json = match serde_json::to_string(&none) {
            Ok(s) => s,
            Err(e) => panic!("serialize none failed: {e}"),
        };
        assert_eq!(none_json, r#"{"ts":null}"#);
        let none_back: Wrapper = match serde_json::from_str(&none_json) {
            Ok(v) => v,
            Err(e) => panic!("deserialize none failed: {e}"),
        };
        assert_eq!(none_back, none);

        let some = Wrapper {
            ts: match "2026-07-09T01:50:48.876Z".parse::<DateTime<Utc>>() {
                Ok(dt) => Some(dt),
                Err(e) => panic!("fixture parse failed: {e}"),
            },
        };
        let some_json = match serde_json::to_string(&some) {
            Ok(s) => s,
            Err(e) => panic!("serialize some failed: {e}"),
        };
        assert!(
            !some_json.contains("Z\""),
            "Option Some 不应输出 Z: {some_json}"
        );
        let some_back: Wrapper = match serde_json::from_str(&some_json) {
            Ok(v) => v,
            Err(e) => panic!("deserialize some failed: {e}"),
        };
        assert_eq!(some_back.ts, some.ts);

        // 兼容旧 Z 数据
        let legacy = r#"{"ts":"2026-07-09T01:50:48.876Z"}"#;
        let legacy_back: Wrapper = match serde_json::from_str(legacy) {
            Ok(v) => v,
            Err(e) => panic!("deserialize legacy failed: {e}"),
        };
        assert_eq!(legacy_back.ts.map(|t| t.timestamp()), Some(1_783_561_848));
    }
}
