#![cfg(feature = "data")]

use echo_core::tools::{Tool, ToolParameters};
use echo_tools::data::{DataExportTool, DataReadTool, is_numeric, load_dataframe};
use polars::prelude::*;
use serde_json::json;
use std::path::Path;

type TestResult = std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn path_text(path: &Path) -> std::io::Result<&str> {
    path.to_str()
        .ok_or_else(|| std::io::Error::other("测试路径必须为 UTF-8"))
}

fn assert_real_zstd_parquet(path: &Path) -> TestResult {
    let mut reader = ParquetReader::new(std::fs::File::open(path)?);
    let metadata = reader.get_metadata()?;
    assert!(!metadata.row_groups.is_empty());
    let mut count = 0_usize;
    for group in &metadata.row_groups {
        for column in group.parquet_columns() {
            // 经 Polars 的原公开 metadata 检查实际 codec，不靠默认值或文件名推定。
            assert_eq!(format!("{:?}", column.compression()), "Zstd");
            count = count.saturating_add(1);
        }
    }
    assert!(count > 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn original_data_tools_preserve_parquet_zstd_unicode_null_and_numbers() -> TestResult {
    let directory = tempfile::tempdir()?;
    let input = directory.path().join("输入.csv");
    let output = directory.path().join("往返🧪.parquet");
    std::fs::write(&input, "名称,数值,备注\n你好🧪,42.5,\ncafé,-7.25,保留\n")?;
    let export = DataExportTool
        .execute(ToolParameters::from([
            ("input_file".into(), json!(path_text(&input)?)),
            ("output_file".into(), json!(path_text(&output)?)),
            ("format".into(), json!("parquet")),
        ]))
        .await?;
    assert!(export.success, "{export:?}");
    assert_real_zstd_parquet(&output)?;

    let frame = load_dataframe(&output, None)?;
    assert_eq!(frame.shape(), (2, 3));
    assert_eq!(frame.column("名称")?.str()?.get(0), Some("你好🧪"));
    assert_eq!(frame.column("名称")?.str()?.get(1), Some("café"));
    assert_eq!(frame.column("数值")?.f64()?.get(0), Some(42.5));
    assert_eq!(frame.column("数值")?.f64()?.get(1), Some(-7.25));
    assert!(is_numeric(frame.column("数值")?.dtype()));
    assert_eq!(frame.column("备注")?.null_count(), 1);

    let read = DataReadTool
        .execute(ToolParameters::from([(
            "file_path".into(),
            json!(path_text(&output)?),
        )]))
        .await?;
    assert!(read.success, "{read:?}");
    let data = read
        .data
        .ok_or_else(|| std::io::Error::other("缺少原工具结构化输出"))?;
    assert_eq!(data.get("rows"), Some(&json!(2)));
    assert_eq!(
        data.get("preview"),
        Some(&json!([
            {"名称":"你好🧪","数值":42.5,"备注":null},
            {"名称":"café","数值":-7.25,"备注":"保留"}
        ]))
    );

    #[cfg(feature = "statistics")]
    {
        let statistics = echo_tools::statistics::ExploratoryStatisticsTool::default()
            .execute(ToolParameters::from([(
                "data_path".into(),
                json!(path_text(&output)?),
            )]))
            .await?;
        assert!(statistics.success, "{statistics:?}");
        assert_eq!(
            statistics.data.as_ref().and_then(|d| d.get("inference")),
            Some(&json!(false))
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn original_data_tools_report_invalid_parquet_and_export_format() -> TestResult {
    let directory = tempfile::tempdir()?;
    let invalid = directory.path().join("损坏.parquet");
    std::fs::write(&invalid, b"PAR1invalid dataPAR1")?;
    assert!(
        DataReadTool
            .execute(ToolParameters::from([(
                "file_path".into(),
                json!(path_text(&invalid)?),
            )]))
            .await
            .is_err()
    );
    let input = directory.path().join("输入.csv");
    let output = directory.path().join("不应创建.parquet");
    std::fs::write(&input, "value\n1\n")?;
    assert!(
        DataExportTool
            .execute(ToolParameters::from([
                ("input_file".into(), json!(path_text(&input)?)),
                ("output_file".into(), json!(path_text(&output)?)),
                ("format".into(), json!("unsupported")),
            ]))
            .await
            .is_err()
    );
    assert!(!output.exists());
    Ok(())
}

#[cfg(feature = "media")]
#[tokio::test(flavor = "multi_thread")]
async fn original_excel_load_writes_readable_zstd_parquet() -> TestResult {
    use echo_tools::excel::{ExcelLoadTool, ExcelWriteTool};

    let directory = tempfile::tempdir()?;
    let input = directory.path().join("工作表.xlsx");
    let output = directory.path().join("表格.parquet");
    let written = ExcelWriteTool
        .execute(ToolParameters::from([
            ("file_path".into(), json!(path_text(&input)?)),
            ("sheet".into(), json!("工作表")),
            ("headers".into(), json!(["名称", "数值"])),
            ("data".into(), json!([["你好🧪", 1.5], ["第二行", 4.0]])),
        ]))
        .await?;
    assert!(written.success, "{written:?}");
    let loaded = ExcelLoadTool
        .execute(ToolParameters::from([
            ("file_path".into(), json!(path_text(&input)?)),
            ("sheet".into(), json!("工作表")),
            ("output_file".into(), json!(path_text(&output)?)),
        ]))
        .await?;
    assert!(loaded.success, "{loaded:?}");
    assert_real_zstd_parquet(&output)?;
    let frame = load_dataframe(&output, None)?;
    assert_eq!(frame.shape(), (2, 2));
    assert_eq!(frame.column("名称")?.str()?.get(0), Some("你好🧪"));
    assert_eq!(
        frame
            .column("数值")?
            .cast(&DataType::Float64)?
            .f64()?
            .get(0),
        Some(1.5)
    );
    Ok(())
}
