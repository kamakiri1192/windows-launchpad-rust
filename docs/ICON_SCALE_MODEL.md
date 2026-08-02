# Icon visual-size model

The launcher can learn the correction between the existing three-category icon
sizing rule and a human-selected visual size. All training and inference stay
local. Runtime inference is pure Rust and does not require Python, ONNX Runtime,
Core ML, DirectML, or an NPU.

## Runtime order

For each newly extracted icon, the worker uses this precedence:

1. Exact manual override for the app id or source path
2. Learned Gradient Boosting model
3. Existing `solid_fill` category rule (`1.00`, `0.92`, or `0.74`)

The final scale is clamped to `0.55..=1.10`. Invalid, incompatible, undersized,
or out-of-domain models are ignored safely.

Inference happens once during icon extraction. The resulting 128×128 RGBA image
is stored in SQLite, so no model work occurs during rendering or animation.

## 1. Generate a calibration workspace

macOS:

```bash
cargo run --example icon_scale_audit -- \
  --out-dir ./icon-scale-audit
```

Windows PowerShell:

```powershell
cargo run --example icon_scale_audit -- `
  --out-dir .\icon-scale-audit
```

The command creates:

- `audit.json`: stable 19-value feature vectors and current rule scales
- `icons/*.png`: launcher-normalized previews
- `calibrate.html`: offline browser UI

Open `calibrate.html`. Select a visually balanced reference icon, adjust each
slider until the icon has the same perceived size, tick **この倍率で確定**, and
export `labels.json`.

At least 12 confirmed icons are required. Fifty or more diverse icons are
recommended. Include rounded-square icons, circles, thin-line logos, irregular
silhouettes, multi-part marks, dark icons, and pale icons.

## 2. Train and install

```bash
cargo run --example icon_scale_train -- \
  --audit ./icon-scale-audit/audit.json \
  --labels ./icon-scale-audit/labels.json \
  --install
```

The trainer writes:

- `icon-scale-model.json`: Gradient Boosting decision-stump ensemble
- `icon-scale-overrides.json`: exact scales for labelled icons
- `training-report.json`: sample count, tree count, and training RMSE

With `--install`, the model is copied to the launcher's application-data folder:

- macOS: `~/Library/Application Support/Launchpad/`
- Windows: `%LOCALAPPDATA%\Launchpad\`

Restart the launcher. It detects the policy revision, invalidates only the baked
icon rows, and re-extracts them with the new scale policy. Removing or replacing
the model files likewise triggers a one-time icon-cache refresh.

## Model representation

The target is the logarithmic correction relative to the current rule:

```text
log(manual_scale / rule_scale)
```

Training uses Gradient Boosting over shallow one-split trees. Each tree compares
one of the 19 deterministic features with a threshold and adds a small correction.
The format is intentionally simple so inference is a short loop over scalar
values and remains fast on ordinary CPUs.

The model stores the feature names and order. A model trained by an incompatible
build is rejected rather than interpreted with the wrong columns.

## Feature set

The fixed feature vector contains:

- alpha coverage at four thresholds
- alpha mass
- bounding-box width, height, and area ratios
- logarithmic aspect ratio
- existing `solid_fill`
- alpha-weighted centroid
- perimeter ratio and circularity
- enclosed-hole ratio
- connected-component count and dominant-component ratio
- alpha-weighted luminance mean and standard deviation

## Reset

To return to the original rule-only behavior, remove both files and restart:

```text
icon-scale-model.json
icon-scale-overrides.json
```

The policy revision marker causes the cached normalized icons to be regenerated
once. No database editing is required.

## Performance

Feature extraction and model inference run in the existing background icon
worker. A typical model has fewer than 100 decision stumps, so inference is much
smaller than icon decoding and Lanczos resizing. M-series Macs and Copilot+ PCs
do not need GPU or NPU acceleration for this workload.