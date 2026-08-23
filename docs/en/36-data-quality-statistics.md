# Data Quality & Statistical Analysis

## Product Model

embedding application is coding-first. Built-in data tools inspect, clean, reshape, summarize, and visualize data. Formal statistical inference is performed by reviewable Python/R code written with the user, executed through the sandboxed `run_code` path, and preserved as task artifacts.

The framework does not implement its own p-value approximations or multivariable regression engine.

## Features

| Feature | Capability |
|---|---|
| `data` | Profiling, filtering, aggregation, joins, pivots, quality checks, correlations, and charts |
| `statistics` | `exploratory_statistics`, a descriptive-only distribution summary |
| `shell` | `run_code` for sandboxed Python/R/JavaScript execution |

`statistics` depends on `data`. Formal inference normally enables both `statistics` and `shell`.

## Tool Map

| Goal | Tool | Contract |
|---|---|---|
| Inspect schema and basic quality | `profile_data` | Types, nulls, distinct values, samples |
| Detailed per-column summary | `data_stats` | Counts, percentiles, variance, ranges |
| Missingness | `missing_value_analysis` | Patterns and bounded recommendations |
| Outliers | `outlier_detection` | IQR/Z-score exploration |
| Consistency | `consistency_check` | Type, range, and rule checks |
| Correlation matrix | `correlate_data` | Exploratory coefficients |
| Distribution summary | `exploratory_statistics` | No p-values, intervals, or significance claims |
| Formal inference/modeling | `run_code` | Persisted SciPy/statsmodels/R script |

## Exploratory Statistics

`exploratory_statistics` returns:

- total, finite, and missing/non-finite counts;
- mean and sample standard deviation;
- minimum, p25, median, p75, and maximum;
- moment skewness and excess kurtosis;
- `inference: false` and explicit limitations.

```json
{
  "tool": "exploratory_statistics",
  "parameters": {
    "data_path": "/workspace/data/cohort.csv",
    "columns": "age,bmi,outcome"
  }
}
```

This tool is appropriate for model selection and data-quality exploration. It must not be used to claim statistical significance, causality, treatment effect, or calibrated uncertainty.

## Formal Inference Workflow

1. Define the estimand, population, comparison, time window, missing-data rule, and model assumptions.
2. Inspect the input with data-quality tools.
3. Write `analysis.py` or `analysis.R` into the task working directory.
4. Use mature libraries such as SciPy/statsmodels or established R packages.
5. Record the input SHA-256, package versions, random seed, parameters, diagnostics, warnings, and result paths.
6. Execute the persisted script through `run_code` with `script_path`; do not
   read it back into a second inline wrapper.
7. Review the code and outputs with the user; edit and rerun the same artifact when needed.

```json
{
  "tool": "run_code",
  "parameters": {
    "language": "python",
    "script_path": "analysis.py"
  }
}
```

Example OLS pattern:

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
    "missing_data": "complete cases for outcome, age, bmi",
}
Path("artifacts").mkdir(exist_ok=True)
Path("artifacts/model.txt").write_text(result.summary().as_text(), encoding="utf-8")
Path("artifacts/manifest.json").write_text(
    json.dumps(manifest, indent=2), encoding="utf-8"
)
```

The exact model, covariance estimator, diagnostics, and sensitivity analysis remain part of the code review rather than a closed tool parameter schema.

## Removed Unsafe Contracts

The former `hypothesis_test`, `regression`, and `descriptive_advanced` tools were removed because they mixed exploratory calculations with inferential claims. In particular, the old regression path combined separate one-feature slopes and described the result as multivariable OLS.

Use `exploratory_statistics` for description and persisted mature-library code for inference.
