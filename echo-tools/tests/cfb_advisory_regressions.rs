#![cfg(feature = "media")]

use calamine::{Reader, Xls, XlsError};
use std::io::Cursor;

/// 上游PR762的短目录样本仍是可读取的工作簿，不能因尾部短块而panic。
#[test]
fn cfb_short_directory_remains_readable() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = include_bytes!("fixtures/cfb-advisory/cfb_dir_short_chunk_panic.xls");
    let workbook = Xls::new(Cursor::new(bytes.as_slice()))?;
    assert_eq!(workbook.sheet_names(), vec!["Sheet1"]);
    Ok(())
}

/// 循环DIFAT必须返回解析错误，避免小文件触发无限内存增长。
#[test]
fn cfb_cyclic_difat_returns_error() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = include_bytes!("fixtures/cfb-advisory/cfb_difat_oom.xls");
    let error = match Xls::new(Cursor::new(bytes.as_slice())) {
        Err(XlsError::Cfb(error)) => error,
        Err(other) => return Err(format!("expected bounded CFB sector error, got {other}").into()),
        Ok(_) => return Err("cyclic DIFAT unexpectedly accepted".into()),
    };
    assert!(error.to_string().ends_with("points past end of file"));
    Ok(())
}
