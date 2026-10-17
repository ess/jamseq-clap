use clack_host::prelude::PluginEntry;
use jamseq_plugin_api::JamSeqPluginType;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub fn scan_clap_plugins() -> Vec<(String, String, String, JamSeqPluginType)> {
    let mut results = Vec::new();
    let mut seen_paths = HashSet::new();

    let scan_paths = get_clap_search_paths();

    for dir in scan_paths {
        if !dir.exists() {
            continue;
        }
        scan_directory_recursive(&dir, &mut results, &mut seen_paths);
    }

    results
}

fn get_clap_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Standard environment variable
    if let Ok(clap_env) = std::env::var("CLAP_PATH") {
        for p in std::env::split_paths(&clap_env) {
            paths.push(p);
        }
    }

    // Standard Linux search paths
    #[cfg(target_os = "linux")]
    {
        paths.push(PathBuf::from("/usr/lib/clap"));
        paths.push(PathBuf::from("/usr/local/lib/clap"));
        if let Some(home) = std::env::var_os("HOME") {
            let home_path = PathBuf::from(home);
            paths.push(home_path.join(".clap"));
            paths.push(home_path.join(".local/lib/clap"));
        }
    }

    // Standard macOS search paths
    #[cfg(target_os = "macos")]
    {
        paths.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
        if let Some(home) = std::env::var_os("HOME") {
            paths.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/CLAP"));
        }
    }

    // Standard Windows search paths
    #[cfg(target_os = "windows")]
    {
        if let Some(common) = std::env::var_os("COMMONPROGRAMFILES") {
            paths.push(PathBuf::from(common).join("CLAP"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            paths.push(PathBuf::from(local).join("Programs/Common/CLAP"));
        }
    }

    // Relative project / bundled directories
    paths.push(PathBuf::from("plugins"));
    paths.push(PathBuf::from("../plugins"));

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join("plugins"));
        }
    }

    paths
}

fn scan_directory_recursive(
    dir: &Path,
    results: &mut Vec<(String, String, String, JamSeqPluginType)>,
    seen_paths: &mut HashSet<String>,
) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Check if directory is a .clap bundle
            if path.extension().and_then(|s| s.to_str()) == Some("clap") {
                inspect_candidate_file(&path, results, seen_paths);
            } else {
                scan_directory_recursive(&path, results, seen_paths);
            }
        } else if path.is_file() {
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if ext == "clap" || ext == "so" || ext == "dll" || ext == "dylib" {
                inspect_candidate_file(&path, results, seen_paths);
            }
        }
    }
}

fn inspect_candidate_file(
    path: &Path,
    results: &mut Vec<(String, String, String, JamSeqPluginType)>,
    seen_paths: &mut HashSet<String>,
) {
    let path_str = path.to_string_lossy().to_string();
    if seen_paths.contains(&path_str) {
        return;
    }

    let entry = match unsafe { PluginEntry::load(&path_str) } {
        Ok(e) => e,
        Err(_) => return,
    };

    let factory = match entry.get_plugin_factory() {
        Some(f) => f,
        None => return,
    };

    let mut found_any = false;
    for desc in factory.plugin_descriptors() {
        let name = desc
            .name()
            .map(|c| c.to_string_lossy().to_string())
            .unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("CLAP Plugin")
                    .to_string()
            });

        let plugin_type = deduce_plugin_type(&desc, &name);
        results.push((name, path_str.clone(), "clap".to_string(), plugin_type));
        found_any = true;
    }

    if found_any {
        seen_paths.insert(path_str);
    }
}

fn deduce_plugin_type(
    desc: &clack_common::plugin::PluginDescriptor,
    display_name: &str,
) -> JamSeqPluginType {
    let mut is_instrument = false;
    let mut is_effect = false;
    let mut is_analyzer = false;

    for feat in desc.features() {
        let f = feat.to_str().unwrap_or("").to_lowercase();
        if f == "instrument" || f == "synthesizer" || f == "sampler" || f == "drum" {
            is_instrument = true;
        } else if f == "audio-effect"
            || f == "filter"
            || f == "reverb"
            || f == "delay"
            || f == "distortion"
            || f == "compressor"
        {
            is_effect = true;
        } else if f == "analyzer" || f == "utility" {
            is_analyzer = true;
        }
    }

    if is_instrument {
        JamSeqPluginType::Instrument
    } else if is_effect {
        JamSeqPluginType::AudioEffect
    } else if is_analyzer {
        JamSeqPluginType::Analyzer
    } else {
        // Fallback heuristics based on name
        let lower = display_name.to_lowercase();
        if lower.contains("effect")
            || lower.contains(" fx")
            || lower.contains("reverb")
            || lower.contains("delay")
            || lower.contains("compressor")
            || lower.contains("filter")
        {
            JamSeqPluginType::AudioEffect
        } else {
            JamSeqPluginType::Instrument
        }
    }
}
