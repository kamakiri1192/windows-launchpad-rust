//! Generate a local visual calibration workspace for the learned icon scaler.
//!
//! macOS:
//!   cargo run --example icon_scale_audit -- --out-dir ./icon-scale-audit
//!
//! Windows:
//!   cargo run --example icon_scale_audit -- --out-dir .\icon-scale-audit
//!
//! Open `calibrate.html`, adjust and confirm icons, then export `labels.json`.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use image::RgbaImage;
use serde::Serialize;

#[cfg(windows)]
use launchpad_windows::domain::app_id::AppId;
use launchpad_windows::icons::features::{self, IconVisualFeatures, FEATURE_NAMES};
use launchpad_windows::icons::normalize::{self, DecodedIcon};
use launchpad_windows::icons::sizing;

#[derive(Debug, Serialize)]
struct AuditReport {
    format_version: u32,
    feature_names: &'static [&'static str],
    entries: Vec<AuditEntry>,
}

#[derive(Debug, Clone, Serialize)]
struct AuditEntry {
    key: String,
    name: String,
    source_path: String,
    icon_path: String,
    category: String,
    rule_scale: f32,
    features: IconVisualFeatures,
    feature_vector: Vec<f32>,
}

#[derive(Debug)]
struct DiscoveredApp {
    key: String,
    name: String,
    source_path: PathBuf,
}

#[derive(Debug)]
struct Args {
    out_dir: PathBuf,
}

fn main() {
    #[cfg(windows)]
    let _com = launchpad_windows::icons::extract::ComScope::new();

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        eprintln!("icon-scale-audit supports Windows and macOS only");
        std::process::exit(1);
    }

    let args = parse_args().unwrap_or_else(|error| {
        eprintln!("error: {error}");
        std::process::exit(2);
    });
    std::fs::create_dir_all(args.out_dir.join("icons")).expect("create audit directory");

    eprintln!("icon-scale-audit: scanning installed applications...");
    let apps = discover_apps();
    eprintln!("  found {} candidates", apps.len());

    let mut entries = Vec::new();
    for (index, app) in apps.iter().enumerate() {
        if let Some(entry) = process_app(app, entries.len(), &args.out_dir) {
            entries.push(entry);
        }
        if (index + 1) % 50 == 0 || index + 1 == apps.len() {
            eprintln!("  {}/{}", index + 1, apps.len());
        }
    }
    entries.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));

    let report = AuditReport {
        format_version: 1,
        feature_names: &FEATURE_NAMES,
        entries,
    };
    let report_json = serde_json::to_string_pretty(&report).expect("serialize audit report");
    std::fs::write(args.out_dir.join("audit.json"), &report_json).expect("write audit report");

    let browser_data = serde_json::to_string(&report.entries)
        .expect("serialize browser data")
        .replace("</", "<\\/");
    let html = CALIBRATOR_HTML.replace("__ICON_DATA__", &browser_data);
    std::fs::write(args.out_dir.join("calibrate.html"), html).expect("write calibrator HTML");

    eprintln!(
        "icon-scale-audit: wrote {} icons to {}",
        report.entries.len(),
        args.out_dir.display()
    );
    eprintln!("  open: {}", args.out_dir.join("calibrate.html").display());
}

fn parse_args() -> Result<Args, String> {
    let mut out_dir = PathBuf::from("./icon-scale-audit");
    let args: Vec<String> = std::env::args().collect();
    let mut index = 1usize;
    while index < args.len() {
        match args[index].as_str() {
            "--out-dir" => {
                index += 1;
                out_dir = PathBuf::from(args.get(index).ok_or("--out-dir requires a path")?);
            }
            "--help" | "-h" => {
                eprintln!(
                    "icon-scale-audit — アイコン、特徴量、校正画面を生成する\n\n\
                     Usage: cargo run --example icon_scale_audit -- [OPTIONS]\n\n\
                     Options:\n\
                       --out-dir <dir>  出力先（既定: ./icon-scale-audit）\n\
                       --help           このヘルプを表示する"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
        index += 1;
    }
    Ok(Args { out_dir })
}

fn process_app(app: &DiscoveredApp, output_index: usize, out_dir: &Path) -> Option<AuditEntry> {
    let source = extract_icon(&app.source_path)?;
    let metrics = sizing::analyze(&source)?;
    let visual_features = features::extract(&source)?;
    let normalized = normalize::normalize(&source);
    let relative_icon_path = format!("icons/{output_index:05}.png");
    let icon = RgbaImage::from_raw(
        normalized.image.w,
        normalized.image.h,
        normalized.image.rgba,
    )?;
    icon.save(out_dir.join(&relative_icon_path)).ok()?;

    Some(AuditEntry {
        key: app.key.clone(),
        name: app.name.clone(),
        source_path: app.source_path.to_string_lossy().into_owned(),
        icon_path: relative_icon_path,
        category: metrics.category.as_str().to_owned(),
        rule_scale: metrics.scale as f32,
        feature_vector: visual_features.as_array().to_vec(),
        features: visual_features,
    })
}

#[cfg(windows)]
fn discover_apps() -> Vec<DiscoveredApp> {
    let mut roots = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        roots.push(PathBuf::from(appdata).join("Microsoft/Windows/Start Menu/Programs"));
    }
    if let Some(programdata) = std::env::var_os("PROGRAMDATA") {
        roots.push(PathBuf::from(programdata).join("Microsoft/Windows/Start Menu/Programs"));
    }

    let mut apps = BTreeMap::<String, DiscoveredApp>::new();
    let mut pending: VecDeque<PathBuf> = roots.into_iter().collect();
    while let Some(directory) = pending.pop_front() {
        let Ok(children) = std::fs::read_dir(directory) else {
            continue;
        };
        for child in children.flatten() {
            let path = child.path();
            let Ok(kind) = child.file_type() else {
                continue;
            };
            if kind.is_dir() && !kind.is_symlink() {
                pending.push_back(path);
                continue;
            }
            let is_link = path
                .extension()
                .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("lnk"));
            if !kind.is_file() || !is_link {
                continue;
            }
            let app_id = AppId::from_link_path(&path);
            let name = path
                .file_stem()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            apps.entry(app_id.as_str().to_owned())
                .or_insert(DiscoveredApp {
                    key: app_id.as_str().to_owned(),
                    name,
                    source_path: path,
                });
        }
    }
    apps.into_values().collect()
}

#[cfg(target_os = "macos")]
fn discover_apps() -> Vec<DiscoveredApp> {
    const MAX_DEPTH: usize = 6;
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("Applications"));
    }
    roots.extend([
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
    ]);

    let mut apps = BTreeMap::<String, DiscoveredApp>::new();
    let mut pending: VecDeque<(PathBuf, usize)> = roots.into_iter().map(|root| (root, 0)).collect();
    while let Some((directory, depth)) = pending.pop_front() {
        let Ok(children) = std::fs::read_dir(directory) else {
            continue;
        };
        for child in children.flatten() {
            let path = child.path();
            let Ok(kind) = child.file_type() else {
                continue;
            };
            let is_app = path.extension().is_some_and(|ext| ext == "app");
            let is_directory = if kind.is_symlink() {
                is_app && path.is_dir()
            } else {
                kind.is_dir()
            };
            if !is_directory {
                continue;
            }
            if is_app {
                let source_path = path.to_string_lossy().into_owned();
                let name = path
                    .file_stem()
                    .map(|value| value.to_string_lossy().into_owned())
                    .unwrap_or_default();
                apps.entry(source_path.clone()).or_insert(DiscoveredApp {
                    key: source_path,
                    name,
                    source_path: path,
                });
            } else if depth < MAX_DEPTH {
                pending.push_back((path, depth + 1));
            }
        }
    }
    apps.into_values().collect()
}

#[cfg(windows)]
fn extract_icon(path: &Path) -> Option<DecodedIcon> {
    launchpad_windows::icons::extract::extract_icon_from_lnk(path)
}

#[cfg(target_os = "macos")]
fn extract_icon(path: &Path) -> Option<DecodedIcon> {
    use objc2::rc::autoreleasepool;
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSWorkspace};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    autoreleasepool(|_| {
        let path = NSString::from_str(&path.to_string_lossy());
        let image = NSWorkspace::sharedWorkspace().iconForFile(&path);
        let mut proposed = NSRect::new(NSPoint::ZERO, NSSize::new(512.0, 512.0));
        let cg_image =
            unsafe { image.CGImageForProposedRect_context_hints(&mut proposed, None, None)? };
        let bitmap = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg_image);
        let properties: objc2::rc::Retained<
            objc2_foundation::NSDictionary<
                objc2_app_kit::NSBitmapImageRepPropertyKey,
                objc2::runtime::AnyObject,
            >,
        > = objc2_foundation::NSDictionary::new();
        let png = unsafe {
            bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)?
        };
        image::load_from_memory(unsafe { png.as_bytes_unchecked() })
            .ok()
            .map(DecodedIcon::from_dynamic)
    })
}

const CALIBRATOR_HTML: &str = r#"<!doctype html>
<html lang="ja">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>アイコン視覚サイズ校正</title>
<style>
:root{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;color-scheme:light dark}
body{margin:0;background:#17181b;color:#f5f5f7}header{position:sticky;top:0;z-index:10;padding:14px 18px;background:#202126ee;backdrop-filter:blur(18px);border-bottom:1px solid #ffffff1f}
.toolbar{display:flex;gap:10px;align-items:center;flex-wrap:wrap}.toolbar input,.toolbar select,.toolbar button{font:inherit;padding:7px 10px;border-radius:8px;border:1px solid #ffffff25;background:#2b2d33;color:inherit}
.reference{display:flex;align-items:center;gap:12px;margin-top:10px;flex-wrap:wrap}.reference-box,.preview{width:148px;height:148px;display:grid;place-items:center;border-radius:28px;background:linear-gradient(145deg,#777,#333);overflow:hidden}.reference-box img,.preview img{width:128px;height:128px;transform-origin:center}
.help{max-width:620px;line-height:1.55;font-size:14px;opacity:.88}#grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(250px,1fr));gap:14px;padding:16px}.card{padding:14px;border:1px solid #ffffff1a;border-radius:18px;background:#24262b}.card.confirmed{border-color:#4bd37b}.title{height:42px;font-weight:650;overflow:hidden}.meta{font-size:12px;opacity:.65;margin-bottom:8px}.controls{display:grid;grid-template-columns:1fr auto;gap:8px;margin-top:10px}.controls input[type=range]{width:100%}.scale{font-variant-numeric:tabular-nums;min-width:52px;text-align:right}.confirm{display:flex;gap:7px;align-items:center;margin-top:8px}.hidden{display:none!important}button.primary{background:#3b72ff}#progress{font-variant-numeric:tabular-nums;font-weight:650}
</style>
</head>
<body>
<header>
<div class="toolbar"><input id="filter" placeholder="アプリ名で絞り込み"><label><input id="onlyPending" type="checkbox">未確定のみ</label><span id="progress"></span><button id="export" class="primary">labels.jsonを書き出す</button><button id="importButton">読み込む</button><input id="importFile" type="file" accept="application/json" hidden></div>
<div class="reference"><div class="reference-box"><img id="referenceImage"></div><label>比較基準 <select id="referenceSelect"></select></label><div class="help">左の基準と各カードのアイコンは、同じ128×128pxの表示領域です。輪郭の端をそろえるのではなく、一覧で見たときの面積・線の太さ・存在感が同程度になるようスライダーを調整し、確定してください。</div></div>
</header>
<main id="grid"></main>
<script>
const DATA=__ICON_DATA__;
const STORAGE_KEY="launchpad-icon-scale-labels-v1:"+location.pathname;
let state={};
try{state=JSON.parse(localStorage.getItem(STORAGE_KEY)||"{}")||{}}catch(_){state={}}
const byKey=new Map(DATA.map(entry=>[entry.key,entry]));
const grid=document.getElementById("grid");
const filter=document.getElementById("filter");
const onlyPending=document.getElementById("onlyPending");
const progress=document.getElementById("progress");
const referenceSelect=document.getElementById("referenceSelect");
const referenceImage=document.getElementById("referenceImage");
function save(){localStorage.setItem(STORAGE_KEY,JSON.stringify(state));updateProgress()}
function updateProgress(){const confirmed=Object.values(state).filter(value=>value.confirmed).length;progress.textContent=`${confirmed} / ${DATA.length} 確定`}
function setReference(){const entry=byKey.get(referenceSelect.value)||DATA[0];if(entry)referenceImage.src=entry.icon_path;localStorage.setItem(STORAGE_KEY+":reference",entry?.key||"")}
for(const entry of DATA){const option=document.createElement("option");option.value=entry.key;option.textContent=entry.name;referenceSelect.append(option)}
referenceSelect.value=localStorage.getItem(STORAGE_KEY+":reference")||DATA.find(entry=>entry.category==="fullbleed")?.key||DATA[0]?.key||"";referenceSelect.addEventListener("change",setReference);setReference();
function makeCard(entry){const saved=state[entry.key]||{};const card=document.createElement("section");card.className="card";card.dataset.name=entry.name.toLowerCase();
const title=document.createElement("div");title.className="title";title.textContent=entry.name;const meta=document.createElement("div");meta.className="meta";meta.textContent=`${entry.category} / rule ${entry.rule_scale.toFixed(3)}`;
const preview=document.createElement("div");preview.className="preview";const img=document.createElement("img");img.src=entry.icon_path;preview.append(img);
const controls=document.createElement("div");controls.className="controls";const slider=document.createElement("input");slider.type="range";slider.min="0.55";slider.max="1.10";slider.step="0.005";slider.value=String(saved.manual_scale??entry.rule_scale);const scale=document.createElement("span");scale.className="scale";controls.append(slider,scale);
const confirmLabel=document.createElement("label");confirmLabel.className="confirm";const checkbox=document.createElement("input");checkbox.type="checkbox";checkbox.checked=!!saved.confirmed;confirmLabel.append(checkbox,document.createTextNode("この倍率で確定"));
function update(){const value=Number(slider.value);scale.textContent=value.toFixed(3);img.style.transform=`scale(${value/entry.rule_scale})`;card.classList.toggle("confirmed",checkbox.checked);state[entry.key]={manual_scale:value,confirmed:checkbox.checked};save()}
slider.addEventListener("input",update);checkbox.addEventListener("change",update);card.append(title,meta,preview,controls,confirmLabel);update();return card}
for(const entry of DATA)grid.append(makeCard(entry));
function applyFilter(){const query=filter.value.trim().toLowerCase();for(const card of grid.children){const hideConfirmed=onlyPending.checked&&card.classList.contains("confirmed");card.classList.toggle("hidden",!card.dataset.name.includes(query)||hideConfirmed)}}
filter.addEventListener("input",applyFilter);onlyPending.addEventListener("change",applyFilter);
document.getElementById("export").addEventListener("click",()=>{const entries=[];for(const [key,value] of Object.entries(state)){if(!value.confirmed)continue;const source=byKey.get(key);entries.push({key,name:source?.name||key,manual_scale:value.manual_scale})}const blob=new Blob([JSON.stringify({format_version:1,entries},null,2)],{type:"application/json"});const link=document.createElement("a");link.href=URL.createObjectURL(blob);link.download="labels.json";link.click();URL.revokeObjectURL(link.href)});
const importFile=document.getElementById("importFile");document.getElementById("importButton").addEventListener("click",()=>importFile.click());importFile.addEventListener("change",async()=>{const file=importFile.files[0];if(!file)return;const payload=JSON.parse(await file.text());for(const label of payload.entries||[]){state[label.key]={manual_scale:Number(label.manual_scale),confirmed:true}}save();location.reload()});
updateProgress();
</script>
</body>
</html>"#;
