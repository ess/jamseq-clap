use clack_extensions::note_name::{NoteNameBuffer, PluginNoteName};
use clack_extensions::params::{ParamInfoBuffer, ParamInfoFlags, PluginParams};
use clack_extensions::timer::PluginTimer;
use clack_host::events::event_types::{
    NoteOffEvent, NoteOnEvent, ParamValueEvent, TransportEvent, TransportFlags,
};
use clack_host::events::EventFlags;
use clack_host::prelude::*;
use clack_host::utils::{BeatTime, SecondsTime};
use jamseq_plugin_api::{
    allocate_c_note_name_list, allocate_c_parameter_list, JamSeqAudioBuffer, JamSeqMidiEvent,
    JamSeqNoteName, JamSeqNoteNameList, JamSeqParamType, JamSeqParameterList,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::host::{JamSeqClapHost, JamSeqClapMainThread, JamSeqClapShared};

const GM_DRUM_MAP: &[(u8, &str)] = &[
    (35, "Acoustic Bass Drum"),
    (36, "Bass Drum 1"),
    (37, "Side Stick"),
    (38, "Acoustic Snare"),
    (39, "Hand Clap"),
    (40, "Electric Snare"),
    (41, "Low Floor Tom"),
    (42, "Closed Hi-Hat"),
    (43, "High Floor Tom"),
    (44, "Pedal Hi-Hat"),
    (45, "Low Tom"),
    (46, "Open Hi-Hat"),
    (47, "Low-Mid Tom"),
    (48, "Hi-Mid Tom"),
    (49, "Crash Cymbal 1"),
    (50, "High Tom"),
    (51, "Ride Cymbal 1"),
    (52, "Chinese Cymbal"),
    (53, "Ride Bell"),
    (54, "Tambourine"),
    (55, "Splash Cymbal"),
    (56, "Cowbell"),
    (57, "Crash Cymbal 2"),
];

#[derive(Clone, Debug)]
pub struct ClapParamInternal {
    pub id: ClapId,
    pub name: String,
    pub raw_name: String,
    pub group: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub value: f32,
    pub param_type: JamSeqParamType,
}

pub struct ClapInstance {
    pub path: String,
    pub processor: Arc<Mutex<StartedPluginAudioProcessor<JamSeqClapHost>>>,
    pub pending_params: Arc<Mutex<Vec<(ClapId, f64)>>>,
    pub params: Arc<Mutex<Vec<ClapParamInternal>>>,
    pub param_index: Arc<Mutex<HashMap<String, usize>>>,
    pub note_names: Arc<Mutex<Vec<JamSeqNoteName>>>,
    pub stop_flag: Arc<AtomicBool>,
    pub timer_thread: Option<std::thread::JoinHandle<()>>,
    pub current_bpm: f64,
}

impl ClapInstance {
    pub fn load(
        path: &str,
        sample_rate: f64,
        max_block_size: u32,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let path_clone = path.to_string();

        let timers = Arc::new(Mutex::new(Vec::new()));
        let timers_clone = timers.clone();
        let rescan_needed = Arc::new(AtomicBool::new(false));
        let rescan_needed_main = rescan_needed.clone();
        let rescan_needed_shared = rescan_needed.clone();
        let note_names_changed = Arc::new(AtomicBool::new(false));
        let note_names_changed_main = note_names_changed.clone();

        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_clone = stop_flag.clone();

        let params_arc = Arc::new(Mutex::new(Vec::new()));
        let param_index_arc = Arc::new(Mutex::new(HashMap::new()));
        let params_init = params_arc.clone();
        let param_index_init = param_index_arc.clone();

        let note_names_arc = Arc::new(Mutex::new(Vec::new()));
        let note_names_init = note_names_arc.clone();

        let (init_tx, init_rx) = std::sync::mpsc::channel();

        let thread_handle = std::thread::Builder::new()
            .name("clap-instance-worker".to_string())
            .spawn(move || {
                let entry = match unsafe { PluginEntry::load(&path_clone) } {
                    Ok(e) => Box::leak(Box::new(e)),
                    Err(e) => {
                        let _ = init_tx.send(Err(format!("Load error: {}", e)));
                        return;
                    }
                };

                let factory = match entry.get_plugin_factory() {
                    Some(f) => f,
                    None => {
                        let _ = init_tx.send(Err("No plugin factory found in CLAP binary".to_string()));
                        return;
                    }
                };

                let desc = match factory.plugin_descriptors().next() {
                    Some(d) => d,
                    None => {
                        let _ = init_tx.send(Err("No plugin descriptors found in CLAP factory".to_string()));
                        return;
                    }
                };

                let plugin_id = match desc.id() {
                    Some(id) => id,
                    None => {
                        let _ = init_tx.send(Err("No plugin identifier found in CLAP descriptor".to_string()));
                        return;
                    }
                };

                let plugin_name = desc
                    .name()
                    .map(|c| c.to_string_lossy().to_string())
                    .unwrap_or_else(|| "CLAP Plugin".to_string());

                let host_info = match HostInfo::new(
                    "JamSeq",
                    "JamBand",
                    "https://codeberg.org/ess/jamband",
                    "1.0.0",
                ) {
                    Ok(h) => h,
                    Err(e) => {
                        let _ = init_tx.send(Err(format!("HostInfo error: {}", e)));
                        return;
                    }
                };

                let mut instance = match PluginInstance::<JamSeqClapHost>::new(
                    |_| JamSeqClapShared {
                        rescan_needed: rescan_needed_shared,
                    },
                    |_| JamSeqClapMainThread {
                        timers,
                        rescan_needed: rescan_needed_main,
                        note_names_changed: note_names_changed_main,
                    },
                    entry,
                    plugin_id,
                    &host_info,
                ) {
                    Ok(inst) => inst,
                    Err(e) => {
                        let _ = init_tx.send(Err(format!("Instantiation error: {}", e)));
                        return;
                    }
                };

                let audio_config = PluginAudioConfiguration {
                    sample_rate,
                    min_frames_count: 32,
                    max_frames_count: max_block_size.max(4096),
                };

                let audio_processor = match instance.activate(|_, _| (), audio_config) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = init_tx.send(Err(format!("Activation error: {}", e)));
                        return;
                    }
                };

                let started_processor = match audio_processor.start_processing() {
                    Ok(p) => p,
                    Err(_) => {
                        let _ = init_tx.send(Err("Failed to start audio processor".to_string()));
                        return;
                    }
                };

                // Introspect parameters and note names after start_processing
                introspect_params(&instance.plugin_handle(), &params_init, &param_index_init);
                introspect_note_names(&instance.plugin_handle(), &plugin_name, &note_names_init);

                let _ = init_tx.send(Ok(started_processor));

                // Background timer and event pump loop
                let timer_ext = instance.plugin_handle().get_extension::<PluginTimer>();
                while !stop_flag_clone.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(20));

                    if let Some(ref timer) = timer_ext {
                        let active_timers = timers_clone.lock().unwrap().clone();
                        for timer_id in active_timers {
                            timer.on_timer(&instance.plugin_handle(), timer_id);
                        }
                    }

                    if rescan_needed.swap(false, Ordering::Relaxed) {
                        introspect_params(
                            &instance.plugin_handle(),
                            &params_init,
                            &param_index_init,
                        );
                    }

                    if note_names_changed.swap(false, Ordering::Relaxed) {
                        introspect_note_names(
                            &instance.plugin_handle(),
                            &plugin_name,
                            &note_names_init,
                        );
                    }
                }
            })?;

        let started_processor = match init_rx.recv()? {
            Ok(p) => p,
            Err(e) => return Err(e.into()),
        };

        Ok(Self {
            path: path.to_string(),
            processor: Arc::new(Mutex::new(started_processor)),
            pending_params: Arc::new(Mutex::new(Vec::new())),
            params: params_arc,
            param_index: param_index_arc,
            note_names: note_names_arc,
            stop_flag,
            timer_thread: Some(thread_handle),
            current_bpm: 120.0,
        })
    }

    pub fn process(
        &mut self,
        inputs: Option<&JamSeqAudioBuffer>,
        outputs: &mut JamSeqAudioBuffer,
        midi_events: &[JamSeqMidiEvent],
    ) {
        if outputs.num_channels == 0 || outputs.num_samples == 0 || outputs.channel_data.is_null()
        {
            return;
        }

        let num_samples = outputs.num_samples as usize;

        let mut input_event_buffer = EventBuffer::new();

        // 1. Convert incoming MIDI events
        for ev in midi_events {
            let status_type = ev.status & 0xF0;
            let channel = (ev.status & 0x0F) as u16;
            let note = ev.data1 as u16;
            let vel = ev.data2;

            if status_type == 0x90 && vel > 0 {
                let clack_ev = NoteOnEvent::new(
                    ev.tick_offset,
                    Pckn::new(0u16, channel, note, 0u32),
                    (vel as f64) / 127.0,
                );
                input_event_buffer.push(&clack_ev);
            } else if status_type == 0x80 || (status_type == 0x90 && vel == 0) {
                let clack_ev = NoteOffEvent::new(ev.tick_offset, Pckn::new(0u16, channel, note, 0u32), 0.0);
                input_event_buffer.push(&clack_ev);
            }
        }

        // 2. Pending parameter changes
        if let Ok(mut pending) = self.pending_params.try_lock() {
            for (id, val) in pending.drain(..) {
                let ev = ParamValueEvent::new(0, id, Pckn::match_all(), val);
                input_event_buffer.push(&ev);
            }
        }

        let input_events = input_event_buffer.as_input();
        let mut output_events_buf = EventBuffer::new();
        let mut output_events = output_events_buf.as_output();

        let transport = TransportEvent {
            header: EventHeader::new_core(0, EventFlags::empty()),
            flags: TransportFlags::HAS_TEMPO | TransportFlags::IS_PLAYING,
            song_pos_beats: BeatTime::from_float(0.0),
            song_pos_seconds: SecondsTime::from_float(0.0),
            tempo: self.current_bpm,
            tempo_inc: 0.0,
            loop_start_beats: BeatTime::from_float(0.0),
            loop_end_beats: BeatTime::from_float(0.0),
            loop_start_seconds: SecondsTime::from_float(0.0),
            loop_end_seconds: SecondsTime::from_float(0.0),
            bar_start: BeatTime::from_float(0.0),
            bar_number: 0,
            time_signature_numerator: 4,
            time_signature_denominator: 4,
        };

        // 3. Audio routing
        unsafe {
            let out_l_ptr = *outputs.channel_data.add(0);
            let out_r_ptr = if outputs.num_channels > 1 {
                *outputs.channel_data.add(1)
            } else {
                out_l_ptr
            };

            let out_l_slice = std::slice::from_raw_parts_mut(out_l_ptr, num_samples);
            let out_r_slice = std::slice::from_raw_parts_mut(out_r_ptr, num_samples);

            let mut out_ports = AudioPorts::with_capacity(2, 1);
            let mut out_audio = out_ports.with_output_buffers([AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_output_only([out_l_slice, out_r_slice]),
            }]);

            if let Some(in_buf) = inputs {
                if in_buf.num_channels > 0
                    && in_buf.num_samples > 0
                    && !in_buf.channel_data.is_null()
                {
                    let in_l_ptr = *in_buf.channel_data.add(0);
                    let in_r_ptr = if in_buf.num_channels > 1 {
                        *in_buf.channel_data.add(1)
                    } else {
                        in_l_ptr
                    };

                    let in_l_slice = std::slice::from_raw_parts_mut(in_l_ptr, num_samples);
                    let in_r_slice = std::slice::from_raw_parts_mut(in_r_ptr, num_samples);

                    let mut in_ports = AudioPorts::with_capacity(2, 1);
                    let in_audio = in_ports.with_input_buffers([AudioPortBuffer {
                        latency: 0,
                        channels: AudioPortBufferType::f32_input_only([
                            InputChannel::variable(in_l_slice),
                            InputChannel::variable(in_r_slice),
                        ]),
                    }]);

                    if let Ok(mut proc) = self.processor.lock() {
                        let _ = proc.process(
                            &in_audio,
                            &mut out_audio,
                            &input_events,
                            &mut output_events,
                            None,
                            Some(&transport),
                        );
                    }
                    return;
                }
            }

            // Input is empty / generator mode
            let in_audio = InputAudioBuffers::empty();
            if let Ok(mut proc) = self.processor.lock() {
                let _ = proc.process(
                    &in_audio,
                    &mut out_audio,
                    &input_events,
                    &mut output_events,
                    None,
                    Some(&transport),
                );
            }
        }
    }

    pub fn get_parameters(&self) -> *mut JamSeqParameterList {
        let params = self.params.lock().unwrap();
        let items: Vec<(String, String, String, f32, f32, f32, f32, JamSeqParamType)> = params
            .iter()
            .map(|p| {
                (
                    p.id.get().to_string(),
                    p.name.clone(),
                    p.group.clone(),
                    p.min,
                    p.max,
                    p.default,
                    p.value,
                    p.param_type,
                )
            })
            .collect();

        allocate_c_parameter_list(&items)
    }

    pub fn set_parameter(&self, param_id_or_name: &str, value: f32) {
        let mut params = self.params.lock().unwrap();
        let index_map = self.param_index.lock().unwrap();

        let idx = if let Ok(numeric_id) = param_id_or_name.parse::<u32>() {
            params.iter().position(|p| p.id.get() == numeric_id)
        } else {
            index_map
                .get(param_id_or_name)
                .or_else(|| index_map.get(&param_id_or_name.to_lowercase()))
                .or_else(|| index_map.get(&param_id_or_name.to_lowercase().replace(' ', "_")))
                .or_else(|| index_map.get(&param_id_or_name.to_lowercase().replace('_', " ")))
                .copied()
        };

        if let Some(i) = idx {
            if let Some(param) = params.get_mut(i) {
                let clamped = value.clamp(param.min, param.max);
                param.value = clamped;
                let id = param.id;
                self.pending_params.lock().unwrap().push((id, clamped as f64));
            }
        }
    }

    pub fn get_note_names(&self) -> *mut JamSeqNoteNameList {
        let notes = self.note_names.lock().unwrap();
        allocate_c_note_name_list(&notes)
    }
}

impl Drop for ClapInstance {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.timer_thread.take() {
            let _ = handle.join();
        }
    }
}

fn introspect_params(
    handle: &PluginMainThreadHandle<'_>,
    params_arc: &Arc<Mutex<Vec<ClapParamInternal>>>,
    param_index_arc: &Arc<Mutex<HashMap<String, usize>>>,
) {
    let params_ext = match handle.get_extension::<PluginParams>() {
        Some(ext) => ext,
        None => return,
    };

    let count = params_ext.count(handle);
    let mut buf = ParamInfoBuffer::new();
    let mut new_params = Vec::new();
    let mut new_index = HashMap::new();

    let existing_values: HashMap<u32, f32> = {
        let p_lock = params_arc.lock().unwrap();
        p_lock.iter().map(|p| (p.id.get(), p.value)).collect()
    };

    for i in 0..count {
        if let Some(info) = params_ext.get_info(handle, i, &mut buf) {
            let raw_name = String::from_utf8_lossy(info.name).to_string();
            let min = info.min_value as f32;
            let max = info.max_value as f32;
            let def = info.default_value as f32;
            let val = existing_values.get(&info.id.get()).copied().unwrap_or_else(|| {
                params_ext
                    .get_value(handle, info.id)
                    .map(|v| v as f32)
                    .unwrap_or(def)
            });

            let mod_path = String::from_utf8_lossy(info.module).to_string();
            let mod_trimmed = mod_path.trim_matches('/').trim();

            let (group, display_name) = if !mod_trimmed.is_empty() {
                (mod_trimmed.replace('/', " > "), raw_name.clone())
            } else if let Some(slash_idx) = raw_name.rfind('/') {
                let g = raw_name[..slash_idx].trim().to_string();
                let n = raw_name[slash_idx + 1..].trim().to_string();
                (
                    if g.is_empty() {
                        "General".to_string()
                    } else {
                        g.replace('/', " > ")
                    },
                    if n.is_empty() { raw_name.clone() } else { n },
                )
            } else if raw_name.contains(':') || raw_name.contains(" - ") {
                if let Some(colon_idx) = raw_name.find(':') {
                    let g = raw_name[..colon_idx].trim().to_string();
                    let n = raw_name[colon_idx + 1..].trim().to_string();
                    (g, n)
                } else {
                    ("General".to_string(), raw_name.clone())
                }
            } else {
                ("General".to_string(), raw_name.clone())
            };

            let group = if group == "General" {
                let n = display_name.to_lowercase();
                if n.contains("cutoff")
                    || n.contains("reso")
                    || n.contains("drive")
                    || n.contains("filter")
                {
                    "Filter".to_string()
                } else if n.contains("attack")
                    || n.contains("decay")
                    || n.contains("sustain")
                    || n.contains("release")
                    || n.contains("env")
                {
                    "Amp Envelope".to_string()
                } else if n.contains("glide")
                    || n.contains("voice")
                    || n.contains("portamento")
                    || n.contains("tuning")
                {
                    "Voice".to_string()
                } else if n.contains("gain")
                    || n.contains("volume")
                    || n.contains("master")
                    || n.contains("pan")
                {
                    "Output".to_string()
                } else {
                    "General".to_string()
                }
            } else {
                group
            };

            let idx = new_params.len();
            let mut register_key = |k: &str| {
                new_index.insert(k.to_string(), idx);
                new_index.insert(k.to_lowercase(), idx);
                new_index.insert(k.to_lowercase().replace(' ', "_"), idx);
                new_index.insert(k.to_lowercase().replace('_', " "), idx);
            };

            register_key(&display_name);
            if raw_name != display_name {
                register_key(&raw_name);
            }

            let is_bypass = info.flags.contains(ParamInfoFlags::IS_BYPASS);
            let is_stepped = info.flags.contains(ParamInfoFlags::IS_STEPPED);
            let is_enum = info.flags.contains(ParamInfoFlags::IS_ENUM);

            let param_type = if is_bypass || (is_stepped && min == 0.0 && max == 1.0) {
                JamSeqParamType::Bool
            } else if is_enum {
                JamSeqParamType::Enum
            } else if is_stepped {
                JamSeqParamType::Int
            } else {
                JamSeqParamType::Float
            };

            new_params.push(ClapParamInternal {
                id: info.id,
                name: display_name,
                raw_name,
                group,
                min,
                max,
                default: def,
                value: val,
                param_type,
            });
        }
    }

    *params_arc.lock().unwrap() = new_params;
    *param_index_arc.lock().unwrap() = new_index;
}

fn introspect_note_names(
    handle: &PluginMainThreadHandle<'_>,
    plugin_name: &str,
    note_names_arc: &Arc<Mutex<Vec<JamSeqNoteName>>>,
) {
    let mut mapped_notes = Vec::new();

    if let Some(note_ext) = handle.get_extension::<PluginNoteName>() {
        let count = note_ext.count(handle);
        let mut buffer = NoteNameBuffer::new();

        for i in 0..count as u32 {
            if let Some(note) = note_ext.get(handle, i, &mut buffer) {
                if let clack_common::events::Match::Specific(key) = note.key {
                    if key <= 127 {
                        let name_str = String::from_utf8_lossy(note.name).trim().to_string();
                        if !name_str.is_empty() {
                            mapped_notes.push(JamSeqNoteName::new(key as u8, &name_str));
                        }
                    }
                }
            }
        }
    }

    // Fallback: If plugin does not support clap.note-name or returns no notes,
    // check if it is a drum kit / percussion machine and provide GM standard map.
    if mapped_notes.is_empty() {
        let lower = plugin_name.to_lowercase();
        if lower.contains("drum") || lower.contains("kit") || lower.contains("pearl") || lower.contains("zeppelin") {
            for &(pitch, name) in GM_DRUM_MAP {
                mapped_notes.push(JamSeqNoteName::new(pitch, name));
            }
        }
    }

    *note_names_arc.lock().unwrap() = mapped_notes;
}
