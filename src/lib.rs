//! `jamseq-clap`: Standalone dynamic CLAP plugin host adapter for Jamband.
//!
//! Implements and exports the binary-stable `JamSeqAdapterVTable` C-ABI contract,
//! providing dynamic discovery, instantiation, voice rendering, parameter mutations,
//! and drum / note name introspection (`clap.note-name`) for CLAP plugins.

pub mod host;
pub mod instance;
pub mod scanner;

use std::ffi::{c_char, c_void, CStr};

use instance::ClapInstance;
use jamseq_plugin_api::{
    allocate_c_plugin_list, free_c_note_name_list, free_c_parameter_list, free_c_plugin_list,
    JamSeqAdapterVTable, JamSeqAudioBuffer, JamSeqMidiEvent, JamSeqNoteNameList,
    JamSeqParameterList, JamSeqPluginList, JAMSEQ_API_VERSION,
};

static FORMAT_ID_CSTR: &[u8] = b"clap\0";

unsafe extern "C" fn clap_format_id() -> *const c_char {
    FORMAT_ID_CSTR.as_ptr() as *const c_char
}

unsafe extern "C" fn clap_handles(path_or_uri: *const c_char) -> bool {
    if path_or_uri.is_null() {
        return false;
    }
    let s = unsafe { CStr::from_ptr(path_or_uri).to_string_lossy() };
    s.ends_with(".clap")
        || s.contains(".clap/")
        || s.contains(".clap\\")
        || s.ends_with(".so")
        || s.ends_with(".dll")
        || s.ends_with(".dylib")
        || s.starts_with("clap:")
}

unsafe extern "C" fn clap_scan() -> *mut JamSeqPluginList {
    let plugins = scanner::scan_clap_plugins();
    allocate_c_plugin_list(&plugins)
}

unsafe extern "C" fn clap_free_plugin_list(list: *mut JamSeqPluginList) {
    unsafe { free_c_plugin_list(list) };
}

unsafe extern "C" fn clap_create_instance(
    path_or_uri: *const c_char,
    sample_rate: f64,
    max_block_size: u32,
) -> *mut c_void {
    if path_or_uri.is_null() {
        return std::ptr::null_mut();
    }
    let path = unsafe { CStr::from_ptr(path_or_uri).to_string_lossy().into_owned() };
    match ClapInstance::load(&path, sample_rate, max_block_size) {
        Ok(instance) => Box::into_raw(Box::new(instance)) as *mut c_void,
        Err(e) => {
            eprintln!("[jamseq-clap] Error loading CLAP plugin '{}': {}", path, e);
            std::ptr::null_mut()
        }
    }
}

unsafe extern "C" fn clap_process(
    instance: *mut c_void,
    inputs: *const JamSeqAudioBuffer,
    outputs: *mut JamSeqAudioBuffer,
    midi_events: *const JamSeqMidiEvent,
    midi_count: u32,
) {
    if instance.is_null() || outputs.is_null() {
        return;
    }
    let inst = unsafe { &mut *(instance as *mut ClapInstance) };
    let in_ref = if inputs.is_null() {
        None
    } else {
        Some(unsafe { &*inputs })
    };
    let out_ref = unsafe { &mut *outputs };

    let events_slice = if midi_count > 0 && !midi_events.is_null() {
        unsafe { std::slice::from_raw_parts(midi_events, midi_count as usize) }
    } else {
        &[]
    };

    inst.process(in_ref, out_ref, events_slice);
}

unsafe extern "C" fn clap_get_parameters(instance: *mut c_void) -> *mut JamSeqParameterList {
    if instance.is_null() {
        return std::ptr::null_mut();
    }
    let inst = unsafe { &*(instance as *const ClapInstance) };
    inst.get_parameters()
}

unsafe extern "C" fn clap_free_parameters(list: *mut JamSeqParameterList) {
    unsafe { free_c_parameter_list(list) };
}

unsafe extern "C" fn clap_set_parameter(
    instance: *mut c_void,
    param_id: *const c_char,
    value: f32,
) {
    if instance.is_null() || param_id.is_null() {
        return;
    }
    let inst = unsafe { &mut *(instance as *mut ClapInstance) };
    let id_str = unsafe { CStr::from_ptr(param_id).to_string_lossy() };
    inst.set_parameter(&id_str, value);
}

unsafe extern "C" fn clap_get_note_names(instance: *mut c_void) -> *mut JamSeqNoteNameList {
    if instance.is_null() {
        return std::ptr::null_mut();
    }
    let inst = unsafe { &*(instance as *const ClapInstance) };
    inst.get_note_names()
}

unsafe extern "C" fn clap_free_note_names(list: *mut JamSeqNoteNameList) {
    unsafe { free_c_note_name_list(list) };
}

unsafe extern "C" fn clap_destroy_instance(instance: *mut c_void) {
    if instance.is_null() {
        return;
    }
    unsafe {
        let _ = Box::from_raw(instance as *mut ClapInstance);
    }
}

pub static CLAP_ADAPTER_VTABLE: JamSeqAdapterVTable = JamSeqAdapterVTable {
    api_version: JAMSEQ_API_VERSION,
    format_id: clap_format_id,
    handles: clap_handles,
    scan: clap_scan,
    free_plugin_list: clap_free_plugin_list,
    create_instance: clap_create_instance,
    process: clap_process,
    get_parameters: clap_get_parameters,
    free_parameters: clap_free_parameters,
    set_parameter: clap_set_parameter,
    get_note_names: clap_get_note_names,
    free_note_names: clap_free_note_names,
    destroy_instance: clap_destroy_instance,
};

/// Exported dynamic library entrypoint symbol.
#[unsafe(no_mangle)]
pub extern "C" fn jamseq_get_host_adapter() -> *const JamSeqAdapterVTable {
    &CLAP_ADAPTER_VTABLE
}
