#![cfg(feature = "media")]

use echo_core::tools::{Tool, ToolParameters};
use echo_tools::{
    excel::{ExcelReadTool, ExcelWriteTool},
    word::WordReadTool,
};
use serde_json::json;

type TestResult = std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>;
fn artifact_dir() -> std::io::Result<tempfile::TempDir> {
    tempfile::tempdir()
}
#[tokio::test]
async fn xlsx_real_write_read_preserves_unicode_numbers_and_bool() -> TestResult {
    let directory = artifact_dir()?;
    let path = directory.path().join("表格往返🧪.xlsx");
    let path_text = path
        .to_str()
        .ok_or_else(|| std::io::Error::other("non UTF-8 artifact path"))?;
    let result = ExcelWriteTool
        .execute(ToolParameters::from([
            ("file_path".into(), json!(path_text)),
            ("sheet".into(), json!("工作表")),
            ("headers".into(), json!(["Unicode列", "数值", "布尔"])),
            ("data".into(), json!([["你好，café 🧪", 42.5, true]])),
        ]))
        .await?;
    assert!(result.success, "actual XLSX writer failed: {result:?}");
    let bytes = std::fs::read(&path)?;
    assert_eq!(bytes.get(..4), Some(b"PK\x03\x04".as_slice()));
    let read = ExcelReadTool
        .execute(ToolParameters::from([
            ("file_path".into(), json!(path_text)),
            ("sheet".into(), json!("工作表")),
        ]))
        .await?;
    assert!(read.success, "actual calamine reader failed: {read:?}");
    for value in ["Unicode列", "你好，café 🧪", "42.5", "true"] {
        assert!(
            read.output.contains(value),
            "missing {value}: {}",
            read.output
        );
    }
    println!(
        "Actual ExcelWriteTool/rust_xlsxwriter -> ExcelReadTool/calamine: {} bytes; Unicode/numeric/bool preserved",
        bytes.len()
    );
    Ok(())
}
#[tokio::test]
async fn docx_real_pack_read_preserves_unicode_content() -> TestResult {
    let directory = artifact_dir()?;
    let path = directory.path().join("文档往返🧪.docx");
    let text = "你好，café 🧪 — DOCX physical roundtrip 42.5";
    docx_rs::Docx::new()
        .add_paragraph(docx_rs::Paragraph::new().add_run(docx_rs::Run::new().add_text(text)))
        .build()
        .pack(std::fs::File::create(&path)?)?;
    let bytes = std::fs::read(&path)?;
    assert_eq!(bytes.get(..4), Some(b"PK\x03\x04".as_slice()));
    let read = WordReadTool
        .execute(ToolParameters::from([(
            "file_path".into(),
            json!(
                path.to_str()
                    .ok_or_else(|| std::io::Error::other("non UTF-8 artifact path"))?
            ),
        )]))
        .await?;
    assert!(read.success, "actual DOCX reader failed: {read:?}");
    assert!(
        read.output.contains(text),
        "DOCX content lost: {}",
        read.output
    );
    println!(
        "Actual docx_rs pack -> WordReadTool/docx_rs read: {} bytes; Unicode/content preserved",
        bytes.len()
    );
    Ok(())
}
