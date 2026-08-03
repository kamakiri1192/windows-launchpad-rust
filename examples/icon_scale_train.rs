//! Train and optionally install the icon visual-size model.
//!
//!   cargo run --example icon_scale_train -- \
//!     --audit ./icon-scale-audit/audit.json \
//!     --labels ./icon-scale-audit/labels.json \
//!     --install

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use launchpad_windows::icons::features::{FEATURE_COUNT, FEATURE_NAMES};
use launchpad_windows::icons::scale_model::{
    default_model_dir, train_model, IconScaleOverride, IconScaleOverrides, TrainingConfig,
    TrainingSample, MAX_SCALE, MIN_SCALE, MODEL_FILE_NAME, OVERRIDES_FILE_NAME,
    OVERRIDES_FORMAT_VERSION,
};

#[derive(Debug)]
struct Args {
    audit_path: PathBuf,
    labels_path: PathBuf,
    out_dir: PathBuf,
    install: bool,
    install_dir: Option<PathBuf>,
    rounds: usize,
    learning_rate: f32,
    write_overrides: bool,
}

#[derive(Debug, Deserialize)]
struct AuditReport {
    format_version: u32,
    feature_names: Vec<String>,
    entries: Vec<AuditEntry>,
}

#[derive(Debug, Deserialize)]
struct AuditEntry {
    key: String,
    name: String,
    rule_scale: f32,
    feature_vector: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct LabelReport {
    format_version: u32,
    entries: Vec<LabelEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct LabelEntry {
    key: String,
    name: String,
    manual_scale: f32,
}

#[derive(Debug, Serialize)]
struct TrainingReport {
    labelled_samples: usize,
    audit_entries: usize,
    ignored_labels: Vec<String>,
    rounds_requested: usize,
    stumps_produced: usize,
    learning_rate: f32,
    training_rmse_log_scale: f32,
    validation_rmse_log_scale: f32,
    overrides_enabled: bool,
    override_entries: usize,
    model_path: String,
    overrides_path: String,
    installed_to: Option<String>,
}

fn main() {
    let args = parse_args().unwrap_or_else(|error| {
        eprintln!("error: {error}");
        std::process::exit(2);
    });

    let audit: AuditReport = read_json(&args.audit_path).unwrap_or_else(|error| {
        eprintln!("failed to read {}: {error}", args.audit_path.display());
        std::process::exit(2);
    });
    let labels: LabelReport = read_json(&args.labels_path).unwrap_or_else(|error| {
        eprintln!("failed to read {}: {error}", args.labels_path.display());
        std::process::exit(2);
    });
    validate_inputs(&audit, &labels).unwrap_or_else(|error| {
        eprintln!("invalid training input: {error}");
        std::process::exit(2);
    });

    let labels_by_key: BTreeMap<String, LabelEntry> = labels
        .entries
        .into_iter()
        .map(|label| (label.key.clone(), label))
        .collect();
    let audit_keys: BTreeSet<&str> = audit
        .entries
        .iter()
        .map(|entry| entry.key.as_str())
        .collect();
    let ignored_labels: Vec<String> = labels_by_key
        .keys()
        .filter(|key| !audit_keys.contains(key.as_str()))
        .cloned()
        .collect();

    let mut samples = Vec::new();
    let mut override_entries = Vec::new();
    for entry in &audit.entries {
        let Some(label) = labels_by_key.get(&entry.key) else {
            continue;
        };
        let mut features = [0.0f32; FEATURE_COUNT];
        features.copy_from_slice(&entry.feature_vector);
        samples.push(TrainingSample {
            features,
            rule_scale: entry.rule_scale,
            manual_scale: label.manual_scale,
        });
        if args.write_overrides {
            override_entries.push(IconScaleOverride {
                key: entry.key.clone(),
                name: if label.name.trim().is_empty() {
                    entry.name.clone()
                } else {
                    label.name.clone()
                },
                scale: label.manual_scale,
            });
        }
    }

    let config = TrainingConfig {
        rounds: args.rounds,
        learning_rate: args.learning_rate,
        ..TrainingConfig::default()
    };
    let model = train_model(&samples, config).unwrap_or_else(|error| {
        eprintln!("training failed: {error}");
        std::process::exit(2);
    });
    let overrides = IconScaleOverrides {
        format_version: OVERRIDES_FORMAT_VERSION,
        entries: override_entries,
    };

    std::fs::create_dir_all(&args.out_dir).expect("create model output directory");
    let model_path = args.out_dir.join(MODEL_FILE_NAME);
    let overrides_path = args.out_dir.join(OVERRIDES_FILE_NAME);
    write_json(&model_path, &model).expect("write model");
    write_json(&overrides_path, &overrides).expect("write overrides");

    let installed_to = if args.install {
        let destination = args.install_dir.unwrap_or_else(default_model_dir);
        std::fs::create_dir_all(&destination).expect("create install directory");
        write_json(&destination.join(MODEL_FILE_NAME), &model).expect("install model");
        // `--no-overrides` intentionally installs an empty, valid file so an
        // older override set cannot remain active by accident.
        write_json(&destination.join(OVERRIDES_FILE_NAME), &overrides).expect("install overrides");
        Some(destination)
    } else {
        None
    };

    let report = TrainingReport {
        labelled_samples: samples.len(),
        audit_entries: audit.entries.len(),
        ignored_labels,
        rounds_requested: args.rounds,
        stumps_produced: model.stumps.len(),
        learning_rate: model.learning_rate,
        training_rmse_log_scale: model.training_rmse,
        validation_rmse_log_scale: model.validation_rmse,
        overrides_enabled: args.write_overrides,
        override_entries: overrides.entries.len(),
        model_path: model_path.to_string_lossy().into_owned(),
        overrides_path: overrides_path.to_string_lossy().into_owned(),
        installed_to: installed_to
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
    };
    write_json(&args.out_dir.join("training-report.json"), &report).expect("write training report");

    eprintln!("icon-scale-train: complete");
    eprintln!("  labelled samples: {}", report.labelled_samples);
    eprintln!("  decision stumps:  {}", report.stumps_produced);
    eprintln!("  training RMSE:     {:.6}", report.training_rmse_log_scale);
    eprintln!("  validation RMSE:   {:.6}", report.validation_rmse_log_scale);
    eprintln!("  exact overrides:   {}", report.override_entries);
    eprintln!("  model:             {}", report.model_path);
    eprintln!("  overrides:         {}", report.overrides_path);
    if let Some(path) = report.installed_to {
        eprintln!("  installed to:      {path}");
        eprintln!("  restart Launchpad; its baked icon cache will be invalidated automatically");
    }
}

fn parse_args() -> Result<Args, String> {
    let mut audit_path = PathBuf::from("./icon-scale-audit/audit.json");
    let mut labels_path = PathBuf::from("./icon-scale-audit/labels.json");
    let mut out_dir = PathBuf::from("./icon-scale-model");
    let mut install = false;
    let mut install_dir = None;
    let mut rounds = TrainingConfig::default().rounds;
    let mut learning_rate = TrainingConfig::default().learning_rate;
    let mut write_overrides = true;
    let args: Vec<String> = std::env::args().collect();
    let mut index = 1usize;

    while index < args.len() {
        match args[index].as_str() {
            "--audit" => {
                index += 1;
                audit_path = PathBuf::from(args.get(index).ok_or("--audit requires a path")?);
            }
            "--labels" => {
                index += 1;
                labels_path = PathBuf::from(args.get(index).ok_or("--labels requires a path")?);
            }
            "--out-dir" => {
                index += 1;
                out_dir = PathBuf::from(args.get(index).ok_or("--out-dir requires a path")?);
            }
            "--install" => install = true,
            "--install-dir" => {
                index += 1;
                install_dir = Some(PathBuf::from(
                    args.get(index).ok_or("--install-dir requires a path")?,
                ));
                install = true;
            }
            "--rounds" => {
                index += 1;
                rounds = args
                    .get(index)
                    .ok_or("--rounds requires an integer")?
                    .parse()
                    .map_err(|_| "--rounds must be an integer")?;
            }
            "--learning-rate" => {
                index += 1;
                learning_rate = args
                    .get(index)
                    .ok_or("--learning-rate requires a number")?
                    .parse()
                    .map_err(|_| "--learning-rate must be a number")?;
            }
            "--no-overrides" => write_overrides = false,
            "--help" | "-h" => {
                eprintln!(
                    "icon-scale-train — ローカルの視覚サイズモデルを学習・導入する\n\n\
                     Usage: cargo run --example icon_scale_train -- [OPTIONS]\n\n\
                     Options:\n\
                       --audit <file>          audit.json のパス\n\
                       --labels <file>         書き出した labels.json のパス\n\
                       --out-dir <dir>         モデル出力先\n\
                       --rounds <n>            Boosting回数（既定: 96）\n\
                       --learning-rate <n>     学習率（既定: 0.08）\n\
                       --no-overrides          個別上書きを無効化しモデルだけ検証する\n\
                       --install               Launchpadのデータフォルダーへ導入する\n\
                       --install-dir <dir>     明示したフォルダーへ導入する\n\
                       --help                  このヘルプを表示する"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
        index += 1;
    }

    Ok(Args {
        audit_path,
        labels_path,
        out_dir,
        install,
        install_dir,
        rounds,
        learning_rate,
        write_overrides,
    })
}

fn validate_inputs(audit: &AuditReport, labels: &LabelReport) -> Result<(), String> {
    if audit.format_version != 1 || labels.format_version != 1 {
        return Err("unsupported audit or label format version".into());
    }
    let expected: Vec<String> = FEATURE_NAMES.iter().map(|name| (*name).into()).collect();
    if audit.feature_names != expected {
        return Err("audit feature names/order do not match this build".into());
    }
    if audit.entries.iter().any(|entry| {
        entry.feature_vector.len() != FEATURE_COUNT
            || entry.feature_vector.iter().any(|value| !value.is_finite())
            || !entry.rule_scale.is_finite()
            || entry.rule_scale <= 0.0
    }) {
        return Err("one or more audit rows contain invalid features or rule scales".into());
    }

    let mut keys = BTreeSet::new();
    for label in &labels.entries {
        if label.key.trim().is_empty() || !keys.insert(label.key.as_str()) {
            return Err(format!("duplicate or empty label key: {:?}", label.key));
        }
        if !label.manual_scale.is_finite()
            || !(MIN_SCALE..=MAX_SCALE).contains(&label.manual_scale)
        {
            return Err(format!(
                "label {:?} has an invalid scale; expected {MIN_SCALE}..={MAX_SCALE}",
                label.key
            ));
        }
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    std::fs::write(path, bytes).map_err(|error| error.to_string())
}
