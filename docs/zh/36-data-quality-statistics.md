# 数据质量与统计分析

## 产品模型

embedding application 以 coding 为主。内置数据工具负责查看、清洗、转换、汇总和可视化；正式统计推断由 Agent 与用户共同编写可审阅的 Python/R 代码，通过沙箱化 `run_code` 执行，并作为任务 artifact 保存。

框架不再自行实现 p 值近似或多元回归引擎。

## Feature

| Feature | 能力 |
|---|---|
| `data` | 数据画像、过滤、聚合、连接、透视、质量检查、相关性和图表 |
| `statistics` | `exploratory_statistics`，仅做描述性分布摘要 |
| `shell` | `run_code`，在沙箱中执行 Python/R/JavaScript |

`statistics` 依赖 `data`。正式推断通常同时启用 `statistics` 和 `shell`。

## 工具一览

| 目标 | 工具 | 合同 |
|---|---|---|
| 查看 schema 和基础质量 | `profile_data` | 类型、空值、去重、样本 |
| 逐列详细摘要 | `data_stats` | 计数、分位数、方差、范围 |
| 缺失值 | `missing_value_analysis` | 缺失模式和有界建议 |
| 异常值 | `outlier_detection` | IQR/Z-score 探索 |
| 一致性 | `consistency_check` | 类型、范围和规则检查 |
| 相关矩阵 | `correlate_data` | 探索性相关系数 |
| 分布摘要 | `exploratory_statistics` | 不输出 p 值、区间或显著性结论 |
| 正式推断和建模 | `run_code` | 保存的 SciPy/statsmodels/R 脚本 |

## 探索性统计

`exploratory_statistics` 返回：

- 总行数、有限数值数、缺失/非有限数；
- 均值和样本标准差；
- 最小值、p25、中位数、p75、最大值；
- 矩估计偏度和超额峰度；
- `inference: false` 和明确限制。

```json
{
  "tool": "exploratory_statistics",
  "parameters": {
    "data_path": "/workspace/data/cohort.csv",
    "columns": "age,bmi,outcome"
  }
}
```

该工具用于数据质量探索和方法选择，不能用于宣称统计显著性、因果关系、治疗效果或校准后的不确定性。

## 正式推断工作流

1. 先定义 estimand、人群、比较、时间窗、缺失值规则和模型假设。
2. 用数据质量工具检查输入。
3. 在任务 working directory 中写入 `analysis.py` 或 `analysis.R`。
4. 使用 SciPy/statsmodels 或成熟 R 包。
5. 记录输入 SHA-256、包版本、随机种子、参数、诊断、告警和结果路径。
6. 通过 `run_code` 的 `script_path` 直接执行已经保存的同一脚本，不要读回后再套一层内联代码。
7. 用户可在对话中审阅、修改并重新运行该 artifact。

```json
{
  "tool": "run_code",
  "parameters": {
    "language": "python",
    "script_path": "analysis.py"
  }
}
```

OLS 示例模式：

```python
from pathlib import Path
import hashlib
import json
import platform

import numpy as np
import pandas as pd
import scipy
import statsmodels
import statsmodels.api as sm

SEED = 20260718
INPUT = Path("data/cohort.csv")
np.random.seed(SEED)

frame = pd.read_csv(INPUT)
model_frame = frame[["outcome", "age", "bmi"]].apply(
    pd.to_numeric, errors="coerce"
).dropna()

design = sm.add_constant(model_frame[["age", "bmi"]], has_constant="add")
result = sm.OLS(model_frame["outcome"], design, missing="raise").fit()

manifest = {
    "input_sha256": hashlib.sha256(INPUT.read_bytes()).hexdigest(),
    "python": platform.python_version(),
    "numpy": np.__version__,
    "pandas": pd.__version__,
    "scipy": scipy.__version__,
    "statsmodels": statsmodels.__version__,
    "seed": SEED,
    "missing_data": "outcome、age、bmi 完整病例",
}
Path("artifacts").mkdir(exist_ok=True)
Path("artifacts/model.txt").write_text(result.summary().as_text(), encoding="utf-8")
Path("artifacts/manifest.json").write_text(
    json.dumps(manifest, indent=2, ensure_ascii=False), encoding="utf-8"
)
```

具体模型、协方差估计、诊断和敏感性分析属于代码审阅内容，不应被限制成封闭的工具参数表。

## 已删除的不可靠合同

旧 `hypothesis_test`、`regression` 和 `descriptive_advanced` 已删除，因为它们混合了探索性计算和正式推断。尤其旧 regression 把多个单特征斜率相加，却对外描述为多元 OLS。

描述性探索使用 `exploratory_statistics`；正式推断使用保存的成熟库代码。
