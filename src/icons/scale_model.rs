//! Trainable visual-size model and runtime policy for application icons.
//!
//! The runtime representation is deliberately small and dependency-free: a
//! gradient-boosted ensemble of decision stumps evaluated over the stable
//! feature vector from [`crate::icons::features`]. Training is also implemented
//! in pure Rust so the repository can produce and consume models without a
//! Python/ONNX runtime.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::features::{IconVisualFeatures, FEATURE_COUNT, FEATURE_NAMES};

pub const MODEL_FORMAT_VERSION: u32 = 1;
pub const OVERRIDES_FORMAT_VERSION: u32 = 1;
pub const MODEL_FILE_NAME: &str = "icon-scale-model.json";
pub const OVERRIDES_FILE_NAME: &str = "icon-scale-overrides.json";
pub const POLICY_REVISION_FILE_NAME: &str = "icon-scale-policy.revision";

pub const MIN_SCALE: f32 = 0.55;
pub const MAX_SCALE: f32 = 1.10;
pub const MIN_TRAINING_SAMPLES: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleSource {
    Rule,
    Model,
    ManualOverride,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleDecision {
    pub rule_scale: f32,
    pub final_scale: f32,
    pub confidence: f32,
    pub source: ScaleSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DecisionStump {
    pub feature_index: usize,
    pub threshold: f32,
    pub left_value: f32,
    pub right_value: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IconScaleModel {
    pub format_version: u32,
    pub feature_names: Vec<String>,
    pub base_log_correction: f32,
    pub learning_rate: f32,
    pub stumps: Vec<DecisionStump>,
    pub training_samples: usize,
    pub training_rmse: f32,
    pub feature_min: Vec<f32>,
    pub feature_max: Vec<f32>,
    pub min_scale: f32,
    pub max_scale: f32,
}

impl IconScaleModel {
    pub fn validate(&self) -> Result<(), String> {
        if self.format_version != MODEL_FORMAT_VERSION {
            return Err(format!(
                "unsupported model format version {}",
                self.format_version
            ));
        }
        let expected_names: Vec<String> = FEATURE_NAMES.iter().map(|name| (*name).into()).collect();
        if self.feature_names != expected_names {
            return Err("model feature names/order do not match this build".into());
        }
        if self.training_samples < MIN_TRAINING_SAMPLES {
            return Err(format!(
                "model has {} samples; at least {MIN_TRAINING_SAMPLES} are required",
                self.training_samples
            ));
        }
        if !self.base_log_correction.is_finite()
            || !self.learning_rate.is_finite()
            || !(0.0..=1.0).contains(&self.learning_rate)
            || self.learning_rate == 0.0
            || !self.training_rmse.is_finite()
            || self.training_rmse < 0.0
        {
            return Err("model contains invalid scalar metadata".into());
        }
        if self.feature_min.len() != FEATURE_COUNT || self.feature_max.len() != FEATURE_COUNT {
            return Err("model feature range length does not match feature count".into());
        }
        if !self.min_scale.is_finite()
            || !self.max_scale.is_finite()
            || self.min_scale < MIN_SCALE
            || self.max_scale > MAX_SCALE
            || self.min_scale >= self.max_scale
        {
            return Err("model scale bounds are invalid".into());
        }
        for (min, max) in self.feature_min.iter().zip(&self.feature_max) {
            if !min.is_finite() || !max.is_finite() || min > max {
                return Err("model feature ranges are invalid".into());
            }
        }
        for stump in &self.stumps {
            if stump.feature_index >= FEATURE_COUNT
                || !stump.threshold.is_finite()
                || !stump.left_value.is_finite()
                || !stump.right_value.is_finite()
            {
                return Err("model contains an invalid decision stump".into());
            }
        }
        Ok(())
    }

    pub fn predict_log_correction(&self, features: &IconVisualFeatures) -> Option<f32> {
        self.validate().ok()?;
        let values = features.as_array();
        if values.iter().any(|value| !value.is_finite()) {
            return None;
        }
        let mut prediction = self.base_log_correction;
        for stump in &self.stumps {
            let leaf = if values[stump.feature_index] <= stump.threshold {
                stump.left_value
            } else {
                stump.right_value
            };
            prediction += self.learning_rate * leaf;
        }
        Some(prediction.clamp(-0.70, 0.70))
    }

    pub fn predict_scale(
        &self,
        features: &IconVisualFeatures,
        rule_scale: f32,
    ) -> Option<(f32, f32)> {
        if !rule_scale.is_finite() || rule_scale <= 0.0 {
            return None;
        }
        let values = features.as_array();
        let in_domain = self.in_domain_ratio(&values);
        if in_domain < 0.55 {
            return None;
        }
        let correction = self.predict_log_correction(features)?;
        let scale = (rule_scale * correction.exp()).clamp(self.min_scale, self.max_scale);
        let sample_confidence = (self.training_samples as f32 / 80.0).clamp(0.25, 1.0);
        let fit_confidence = (1.0 - self.training_rmse / 0.18).clamp(0.0, 1.0);
        let confidence = (in_domain * sample_confidence * fit_confidence).clamp(0.0, 1.0);
        Some((scale, confidence))
    }

    fn in_domain_ratio(&self, values: &[f32; FEATURE_COUNT]) -> f32 {
        let inside = values
            .iter()
            .zip(self.feature_min.iter().zip(&self.feature_max))
            .filter(|(value, (min, max))| {
                let span = (**max - **min).abs();
                let margin = (span * 0.12).max(0.01);
                **value >= **min - margin && **value <= **max + margin
            })
            .count();
        inside as f32 / FEATURE_COUNT as f32
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IconScaleOverride {
    pub key: String,
    pub name: String,
    pub scale: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IconScaleOverrides {
    pub format_version: u32,
    pub entries: Vec<IconScaleOverride>,
}

impl Default for IconScaleOverrides {
    fn default() -> Self {
        Self {
            format_version: OVERRIDES_FORMAT_VERSION,
            entries: Vec::new(),
        }
    }
}

impl IconScaleOverrides {
    pub fn validate(&self) -> Result<(), String> {
        if self.format_version != OVERRIDES_FORMAT_VERSION {
            return Err(format!(
                "unsupported overrides format version {}",
                self.format_version
            ));
        }
        for entry in &self.entries {
            if entry.key.trim().is_empty()
                || !entry.scale.is_finite()
                || !(MIN_SCALE..=MAX_SCALE).contains(&entry.scale)
            {
                return Err(format!("invalid override for {:?}", entry.key));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScalePolicy {
    model: Option<IconScaleModel>,
    overrides: BTreeMap<String, f32>,
    revision: u64,
}

impl ScalePolicy {
    pub fn from_parts(
        model: Option<IconScaleModel>,
        overrides: IconScaleOverrides,
    ) -> Result<Self, String> {
        if let Some(model) = &model {
            model.validate()?;
        }
        overrides.validate()?;
        let override_map = overrides
            .entries
            .iter()
            .map(|entry| (entry.key.clone(), entry.scale))
            .collect();
        let model_bytes = serde_json::to_vec(&model).map_err(|error| error.to_string())?;
        let override_bytes = serde_json::to_vec(&overrides).map_err(|error| error.to_string())?;
        let revision = hash_policy_bytes(Some(&model_bytes), Some(&override_bytes));
        Ok(Self {
            model,
            overrides: override_map,
            revision,
        })
    }

    pub fn load_from_dir(directory: &Path) -> Self {
        let model_path = directory.join(MODEL_FILE_NAME);
        let overrides_path = directory.join(OVERRIDES_FILE_NAME);
        let model_bytes = std::fs::read(&model_path).ok();
        let overrides_bytes = std::fs::read(&overrides_path).ok();

        let model = model_bytes.as_deref().and_then(
            |bytes| match serde_json::from_slice::<IconScaleModel>(bytes)
                .map_err(|error| error.to_string())
                .and_then(|model| {
                    model.validate()?;
                    Ok(model)
                }) {
                Ok(model) => Some(model),
                Err(error) => {
                    eprintln!(
                        "icon-scale: ignoring invalid model {}: {error}",
                        model_path.display()
                    );
                    None
                }
            },
        );

        let overrides = overrides_bytes
            .as_deref()
            .and_then(|bytes| {
                match serde_json::from_slice::<IconScaleOverrides>(bytes)
                    .map_err(|error| error.to_string())
                    .and_then(|overrides| {
                        overrides.validate()?;
                        Ok(overrides)
                    }) {
                    Ok(overrides) => Some(overrides),
                    Err(error) => {
                        eprintln!(
                            "icon-scale: ignoring invalid overrides {}: {error}",
                            overrides_path.display()
                        );
                        None
                    }
                }
            })
            .unwrap_or_default();

        let override_map = overrides
            .entries
            .iter()
            .map(|entry| (entry.key.clone(), entry.scale))
            .collect();
        let revision = hash_policy_bytes(model_bytes.as_deref(), overrides_bytes.as_deref());

        Self {
            model,
            overrides: override_map,
            revision,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn has_model(&self) -> bool {
        self.model.is_some()
    }

    pub fn override_count(&self) -> usize {
        self.overrides.len()
    }

    pub fn decide(
        &self,
        app_id: &str,
        source_path: &str,
        features: Option<&IconVisualFeatures>,
        rule_scale: f32,
    ) -> ScaleDecision {
        if let Some(scale) = self
            .overrides
            .get(app_id)
            .or_else(|| self.overrides.get(source_path))
            .copied()
        {
            return ScaleDecision {
                rule_scale,
                final_scale: scale.clamp(MIN_SCALE, MAX_SCALE),
                confidence: 1.0,
                source: ScaleSource::ManualOverride,
            };
        }

        if let (Some(model), Some(features)) = (&self.model, features) {
            if let Some((scale, confidence)) = model.predict_scale(features, rule_scale) {
                return ScaleDecision {
                    rule_scale,
                    final_scale: scale,
                    confidence,
                    source: ScaleSource::Model,
                };
            }
        }

        ScaleDecision {
            rule_scale,
            final_scale: rule_scale.clamp(MIN_SCALE, MAX_SCALE),
            confidence: 0.0,
            source: ScaleSource::Rule,
        }
    }
}

pub fn default_model_dir() -> PathBuf {
    #[cfg(windows)]
    {
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Launchpad");
    }
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/Application Support/Launchpad");
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        PathBuf::from(".")
    }
}

fn hash_policy_bytes(model: Option<&[u8]>, overrides: Option<&[u8]>) -> u64 {
    if model.is_none() && overrides.is_none() {
        return 0;
    }
    let mut hash = 0xcbf29ce484222325u64;
    for bytes in [model.unwrap_or_default(), overrides.unwrap_or_default()] {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[derive(Debug, Clone)]
pub struct TrainingSample {
    pub features: [f32; FEATURE_COUNT],
    pub rule_scale: f32,
    pub manual_scale: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct TrainingConfig {
    pub rounds: usize,
    pub learning_rate: f32,
    pub min_samples_leaf: usize,
    pub min_improvement: f32,
}

impl Default for TrainingConfig {
    fn default() -> Self {
        Self {
            rounds: 96,
            learning_rate: 0.08,
            min_samples_leaf: 3,
            min_improvement: 1.0e-7,
        }
    }
}

pub fn train_model(
    samples: &[TrainingSample],
    config: TrainingConfig,
) -> Result<IconScaleModel, String> {
    if samples.len() < MIN_TRAINING_SAMPLES {
        return Err(format!(
            "{} labelled icons supplied; at least {MIN_TRAINING_SAMPLES} are required",
            samples.len()
        ));
    }
    if config.rounds == 0
        || !config.learning_rate.is_finite()
        || !(0.0..=1.0).contains(&config.learning_rate)
        || config.learning_rate == 0.0
        || config.min_samples_leaf == 0
    {
        return Err("invalid training configuration".into());
    }
    for sample in samples {
        if sample.features.iter().any(|value| !value.is_finite())
            || !sample.rule_scale.is_finite()
            || sample.rule_scale <= 0.0
            || !sample.manual_scale.is_finite()
            || !(MIN_SCALE..=MAX_SCALE).contains(&sample.manual_scale)
        {
            return Err("training data contains invalid values".into());
        }
    }

    let targets: Vec<f32> = samples
        .iter()
        .map(|sample| (sample.manual_scale / sample.rule_scale).ln())
        .collect();
    let base = mean(&targets);
    let mut predictions = vec![base; samples.len()];
    let mut stumps = Vec::new();

    for _ in 0..config.rounds {
        let residuals: Vec<f32> = targets
            .iter()
            .zip(&predictions)
            .map(|(target, prediction)| target - prediction)
            .collect();
        let unsplit_loss = squared_error(&residuals);
        let mut best: Option<(f32, DecisionStump)> = None;

        for feature_index in 0..FEATURE_COUNT {
            let mut rows: Vec<(f32, f32)> = samples
                .iter()
                .zip(&residuals)
                .map(|(sample, residual)| (sample.features[feature_index], *residual))
                .collect();
            rows.sort_by(|left, right| left.0.total_cmp(&right.0));

            let total_sum: f32 = rows.iter().map(|row| row.1).sum();
            let total_sq: f32 = rows.iter().map(|row| row.1 * row.1).sum();
            let mut left_sum = 0.0f32;
            let mut left_sq = 0.0f32;

            for split in 1..rows.len() {
                let residual = rows[split - 1].1;
                left_sum += residual;
                left_sq += residual * residual;

                let left_count = split;
                let right_count = rows.len() - split;
                if left_count < config.min_samples_leaf
                    || right_count < config.min_samples_leaf
                    || rows[split - 1].0 == rows[split].0
                {
                    continue;
                }

                let right_sum = total_sum - left_sum;
                let right_sq = total_sq - left_sq;
                let loss = group_squared_error(left_sum, left_sq, left_count)
                    + group_squared_error(right_sum, right_sq, right_count);
                if best
                    .as_ref()
                    .is_some_and(|(best_loss, _)| loss >= *best_loss)
                {
                    continue;
                }

                let threshold = (rows[split - 1].0 + rows[split].0) * 0.5;
                let left_value = (left_sum / left_count as f32).clamp(-0.30, 0.30);
                let right_value = (right_sum / right_count as f32).clamp(-0.30, 0.30);
                best = Some((
                    loss,
                    DecisionStump {
                        feature_index,
                        threshold,
                        left_value,
                        right_value,
                    },
                ));
            }
        }

        let Some((best_loss, stump)) = best else {
            break;
        };
        if unsplit_loss - best_loss < config.min_improvement {
            break;
        }

        for (sample, prediction) in samples.iter().zip(&mut predictions) {
            let leaf = if sample.features[stump.feature_index] <= stump.threshold {
                stump.left_value
            } else {
                stump.right_value
            };
            *prediction += config.learning_rate * leaf;
        }
        stumps.push(stump);
    }

    let residuals: Vec<f32> = targets
        .iter()
        .zip(&predictions)
        .map(|(target, prediction)| target - prediction)
        .collect();
    let training_rmse = (squared_error(&residuals) / samples.len() as f32).sqrt();
    let feature_min = (0..FEATURE_COUNT)
        .map(|index| {
            samples
                .iter()
                .map(|sample| sample.features[index])
                .fold(f32::INFINITY, f32::min)
        })
        .collect();
    let feature_max = (0..FEATURE_COUNT)
        .map(|index| {
            samples
                .iter()
                .map(|sample| sample.features[index])
                .fold(f32::NEG_INFINITY, f32::max)
        })
        .collect();

    let model = IconScaleModel {
        format_version: MODEL_FORMAT_VERSION,
        feature_names: FEATURE_NAMES.iter().map(|name| (*name).into()).collect(),
        base_log_correction: base,
        learning_rate: config.learning_rate,
        stumps,
        training_samples: samples.len(),
        training_rmse,
        feature_min,
        feature_max,
        min_scale: MIN_SCALE,
        max_scale: MAX_SCALE,
    };
    model.validate()?;
    Ok(model)
}

fn mean(values: &[f32]) -> f32 {
    values.iter().sum::<f32>() / values.len() as f32
}

fn squared_error(values: &[f32]) -> f32 {
    values.iter().map(|value| value * value).sum()
}

fn group_squared_error(sum: f32, sum_sq: f32, count: usize) -> f32 {
    (sum_sq - sum * sum / count as f32).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feature(value: f32) -> IconVisualFeatures {
        IconVisualFeatures {
            source_coverage_10: value,
            source_coverage_64: 0.5,
            source_coverage_128: 0.5,
            source_coverage_224: 0.5,
            alpha_mass: 0.5,
            bbox_width_ratio: 1.0,
            bbox_height_ratio: 1.0,
            bbox_area_ratio: 1.0,
            aspect_ratio_log: 0.0,
            solid_fill: 0.5,
            centroid_x: 0.5,
            centroid_y: 0.5,
            perimeter_ratio: 1.0,
            circularity: 0.7,
            hole_ratio: 0.0,
            connected_components: 1.0,
            dominant_component_ratio: 1.0,
            mean_luminance: 0.5,
            luminance_stddev: 0.0,
        }
    }

    #[test]
    fn manual_override_wins_without_a_model() {
        let policy = ScalePolicy::from_parts(
            None,
            IconScaleOverrides {
                format_version: OVERRIDES_FORMAT_VERSION,
                entries: vec![IconScaleOverride {
                    key: "app".into(),
                    name: "App".into(),
                    scale: 0.83,
                }],
            },
        )
        .unwrap();
        let decision = policy.decide("app", "path", Some(&feature(0.5)), 0.74);
        assert_eq!(decision.source, ScaleSource::ManualOverride);
        assert_eq!(decision.final_scale, 0.83);
    }

    #[test]
    fn training_learns_opposite_corrections() {
        let samples: Vec<TrainingSample> = (0..24)
            .map(|index| {
                let value = index as f32 / 23.0;
                let manual_scale = if value < 0.5 { 0.68 } else { 0.82 };
                TrainingSample {
                    features: feature(value).as_array(),
                    rule_scale: 0.74,
                    manual_scale,
                }
            })
            .collect();
        let model = train_model(&samples, TrainingConfig::default()).unwrap();
        let low = model.predict_scale(&feature(0.2), 0.74).unwrap().0;
        let high = model.predict_scale(&feature(0.8), 0.74).unwrap().0;
        assert!(low < 0.74);
        assert!(high > 0.74);
        assert!(high - low > 0.08);
    }

    #[test]
    fn model_round_trips_through_json() {
        let samples: Vec<TrainingSample> = (0..12)
            .map(|index| TrainingSample {
                features: feature(index as f32 / 11.0).as_array(),
                rule_scale: 0.74,
                manual_scale: 0.78,
            })
            .collect();
        let model = train_model(&samples, TrainingConfig::default()).unwrap();
        let json = serde_json::to_vec(&model).unwrap();
        let restored: IconScaleModel = serde_json::from_slice(&json).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored, model);
    }
}
