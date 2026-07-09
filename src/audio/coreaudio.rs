//! Minimal CoreAudio device enumeration — the honest source of truth for the
//! earphone gate on macOS.
//!
//! We read each device's UID, transport type, and output-stream count directly
//! from the HAL rather than trusting the display name, because AirPods enumerate
//! as TWO devices with the *same* name (one HFP input, one A2DP output). Only the
//! UID (stable across reconnect, embeds the BT MAC) plus "has > 0 output streams"
//! disambiguates the earphone we actually play through.
//!
//! Selectors/scopes are defined as explicit four-char codes rather than pulled
//! from `coreaudio-sys` constants, whose Rust names drift across SDK versions.

use std::ffi::c_void;
use std::ptr;

use anyhow::{Result, bail};
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use coreaudio_sys::{
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress,
};

use super::probe::{DeviceInfo, Transport};

/// Build a u32 four-char-code, e.g. `fourcc(b"uid ")`.
const fn fourcc(b: &[u8; 4]) -> u32 {
    ((b[0] as u32) << 24) | ((b[1] as u32) << 16) | ((b[2] as u32) << 8) | (b[3] as u32)
}

const SYSTEM_OBJECT: AudioObjectID = 1; // kAudioObjectSystemObject

const SCOPE_GLOBAL: u32 = fourcc(b"glob");
const SCOPE_OUTPUT: u32 = fourcc(b"outp");
const ELEMENT_MAIN: u32 = 0;

const PROP_DEVICES: u32 = fourcc(b"dev#"); // kAudioHardwarePropertyDevices
const PROP_DEFAULT_OUTPUT: u32 = fourcc(b"dOut"); // kAudioHardwarePropertyDefaultOutputDevice
const PROP_NAME: u32 = fourcc(b"lnam"); // kAudioObjectPropertyName
const PROP_UID: u32 = fourcc(b"uid "); // kAudioDevicePropertyDeviceUID
const PROP_TRANSPORT: u32 = fourcc(b"tran"); // kAudioDevicePropertyTransportType
const PROP_STREAMS: u32 = fourcc(b"stm#"); // kAudioDevicePropertyStreams

const TRANSPORT_BLUETOOTH: u32 = fourcc(b"blue");
const TRANSPORT_BLUETOOTH_LE: u32 = fourcc(b"blea");
const TRANSPORT_BUILTIN: u32 = fourcc(b"bltn");
const TRANSPORT_USB: u32 = fourcc(b"usb ");
const TRANSPORT_AGGREGATE: u32 = fourcc(b"aggr");
const TRANSPORT_VIRTUAL: u32 = fourcc(b"virt");
const TRANSPORT_AIRPLAY: u32 = fourcc(b"airp");
const TRANSPORT_HDMI: u32 = fourcc(b"hdmi");
const TRANSPORT_DISPLAYPORT: u32 = fourcc(b"dprt");

fn addr(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: ELEMENT_MAIN,
    }
}

/// Map a CoreAudio transport fourcc to the platform-agnostic `Transport` enum.
fn transport_from_fourcc(code: u32) -> Transport {
    match code {
        TRANSPORT_BLUETOOTH => Transport::Bluetooth,
        TRANSPORT_BLUETOOTH_LE => Transport::BluetoothLE,
        TRANSPORT_BUILTIN => Transport::BuiltIn,
        TRANSPORT_USB => Transport::Usb,
        TRANSPORT_AGGREGATE => Transport::Aggregate,
        TRANSPORT_VIRTUAL => Transport::Virtual,
        TRANSPORT_AIRPLAY => Transport::AirPlay,
        TRANSPORT_HDMI => Transport::Hdmi,
        TRANSPORT_DISPLAYPORT => Transport::DisplayPort,
        _ => Transport::Other,
    }
}

/// Read an array-valued HAL property (device lists, streams).
unsafe fn read_array<T: Copy>(
    id: AudioObjectID,
    address: &AudioObjectPropertyAddress,
) -> Result<Vec<T>> {
    let mut size: u32 = 0;
    let st = unsafe { AudioObjectGetPropertyDataSize(id, address, 0, ptr::null(), &mut size) };
    if st != 0 {
        bail!("AudioObjectGetPropertyDataSize failed (OSStatus {st})");
    }
    let count = size as usize / std::mem::size_of::<T>();
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut buf: Vec<T> = Vec::with_capacity(count);
    let mut io_size = size;
    let st = unsafe {
        AudioObjectGetPropertyData(
            id,
            address,
            0,
            ptr::null(),
            &mut io_size,
            buf.as_mut_ptr() as *mut c_void,
        )
    };
    if st != 0 {
        bail!("AudioObjectGetPropertyData failed (OSStatus {st})");
    }
    unsafe { buf.set_len(count) };
    Ok(buf)
}

/// Read a scalar HAL property (transport type, default-device id).
unsafe fn read_scalar<T: Copy + Default>(
    id: AudioObjectID,
    address: &AudioObjectPropertyAddress,
) -> Result<T> {
    let mut val = T::default();
    let mut io_size = std::mem::size_of::<T>() as u32;
    let st = unsafe {
        AudioObjectGetPropertyData(
            id,
            address,
            0,
            ptr::null(),
            &mut io_size,
            &mut val as *mut T as *mut c_void,
        )
    };
    if st != 0 {
        bail!("AudioObjectGetPropertyData scalar failed (OSStatus {st})");
    }
    Ok(val)
}

/// Read a CFString HAL property (name, UID) into an owned Rust `String`.
unsafe fn read_cfstring(id: AudioObjectID, selector: u32) -> Result<String> {
    let address = addr(selector, SCOPE_GLOBAL);
    // The property value is a single CFStringRef (pointer-sized). We store it as
    // an opaque pointer, then hand it to core-foundation under the *create* rule
    // (the HAL gives us ownership; CFString drops it with CFRelease).
    let mut cfref: *const c_void = ptr::null();
    let mut io_size = std::mem::size_of::<*const c_void>() as u32;
    let st = unsafe {
        AudioObjectGetPropertyData(
            id,
            &address,
            0,
            ptr::null(),
            &mut io_size,
            &mut cfref as *mut *const c_void as *mut c_void,
        )
    };
    if st != 0 || cfref.is_null() {
        bail!("AudioObjectGetPropertyData cfstring failed (OSStatus {st})");
    }
    let s = unsafe { CFString::wrap_under_create_rule(cfref as _) };
    Ok(s.to_string())
}

/// Enumerate every audio device CoreAudio knows about, annotated with the facts
/// the earphone gate needs. Output is the platform-agnostic `DeviceInfo` so callers
/// (probe command, setup wizard, poll_gate) share one type across targets.
pub fn enumerate() -> Result<Vec<DeviceInfo>> {
    unsafe {
        let default_output: AudioObjectID =
            read_scalar(SYSTEM_OBJECT, &addr(PROP_DEFAULT_OUTPUT, SCOPE_GLOBAL)).unwrap_or(0);

        let ids: Vec<AudioObjectID> = read_array(SYSTEM_OBJECT, &addr(PROP_DEVICES, SCOPE_GLOBAL))?;

        let mut devices = Vec::with_capacity(ids.len());
        for id in ids {
            // A device with no readable name is unusable to us; skip it.
            let Ok(name) = read_cfstring(id, PROP_NAME) else {
                continue;
            };
            let uid = read_cfstring(id, PROP_UID).unwrap_or_else(|_| "?".to_string());
            let transport_code: u32 =
                read_scalar(id, &addr(PROP_TRANSPORT, SCOPE_GLOBAL)).unwrap_or(0);
            let out_streams = read_array::<AudioObjectID>(id, &addr(PROP_STREAMS, SCOPE_OUTPUT))
                .map(|v| v.len())
                .unwrap_or(0);

            devices.push(DeviceInfo {
                stable_id: uid,
                name,
                transport: transport_from_fourcc(transport_code),
                out_streams,
                is_default_output: id == default_output,
            });
        }
        Ok(devices)
    }
}
