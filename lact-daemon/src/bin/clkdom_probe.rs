//! Read-only probe for the clock domain board object group on this GPU.
//!
//! Plumbing is the daemon's own: the same `DriverHandle` sequence (root client
//! over nvidiactl, device/subdevice allocation, NVOS54 controls) with the same
//! layouts as `gpu_controller/nvidia/driver.rs`, whose compile-time size
//! asserts match the layouts verified on hardware in upstream issue #1147.
//!
//! Without arguments this only reads: domain info, control block, per-domain
//! physical clocks via CLK_MEASURE_FREQ, and the propagation topology.
//!
//! The one write path is explicit:
//!
//!     clkdom_probe --write-test <domain-id> <microvolts>
//!
//! It mirrors the daemon's own discipline: read the whole control block first,
//! change one rail field, submit, require exact readback, restore the full
//! preimage. The single command arg sets rail index 1 (MSVDD). A successful
//! write is not proof the board acted on it; CLK_MEASURE_FREQ runs before and
//! after so a physical delta is visible.
//!
//! Domain ids follow the NvAPI clock domain enum as decoded on the upstream
//! threads (0 GPC, 1 XBAR, 2 SYS, 4 memory, 20 Pwr). The names are
//! Blackwell-derived and stay labels, not claims, on other generations.

use std::{
    fs::File,
    mem,
    os::fd::{AsRawFd, OwnedFd, RawFd},
};

use nix::{ioctl_readwrite, unistd::geteuid};

const NV_IOCTL_MAGIC: u8 = 70;
const NV_ESC_RM_ALLOC: u32 = 43;
const NV_ESC_RM_CONTROL: u32 = 42;
const NV_ESC_REGISTER_FD: u32 = 201;

const NV01_DEVICE_0: u32 = 128;
const NV20_SUBDEVICE_0: u32 = 8320;

const CMD_CLK_CLK_DOMAINS_GET_INFO: u32 = 0x2080_9019;
const CMD_CLK_CLK_DOMAINS_GET_CONTROL: u32 = 0x2080_901b;
const CMD_CLK_CLK_DOMAINS_SET_CONTROL: u32 = 0x2080_d01c;
const CMD_CLK_TOP_PROP_RELS_GET_INFO: u32 = 0x2080_9081;
const CMD_CLK_TOP_PROP_RELS_GET_CONTROL: u32 = 0x2080_9083;
#[allow(dead_code)] // documents the SET side; the probe writes no topology values
const CMD_CLK_TOP_PROP_RELS_SET_CONTROL: u32 = 0x2080_d084;
const CMD_CLK_MEASURE_FREQ: u32 = 0x2080_9006;

const CLK_DOMAIN_COUNT: usize = 32;
const CLK_PROP_REL_COUNT: usize = 32;
const MSVDD_RAIL_INDEX: usize = 1;

/// The one layout the threads verified on hardware (Blackwell, R610). The
/// pre-Blackwell info layout is the one the PR handles via a 4-byte shift;
/// both are decoded here so the raw truth of this card is visible.
const TYPE_35: u8 = 0x10;
const TYPE_3X: u8 = 0x0b;
const RANGE_OFFSET_35: usize = 0x26;
const RANGE_OFFSET_3X: usize = 0x22;

fn domain_name(id: u32) -> &'static str {
    match id {
        0 => "GPC",
        1 => "XBAR",
        2 => "SYS",
        4 => "MEMORY",
        5 => "HUB",
        8 => "DISPLAY",
        12 => "LEGACY",
        13 => "MSD",
        20 => "PWR",
        _ => "?",
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NvOs21 {
    h_root: u32,
    h_object_parent: u32,
    h_object_new: u32,
    h_class: u32,
    p_alloc_parms: u64,
    params_size: u32,
    status: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NvOs54 {
    h_client: u32,
    h_object: u32,
    cmd: u32,
    flags: u32,
    params: u64,
    params_size: u32,
    status: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NvOs64 {
    h_root: u32,
    h_object_parent: u32,
    h_object_new: u32,
    h_class: u32,
    p_alloc_parms: u64,
    p_rights_requested: u64,
    params_size: u32,
    flags: u32,
    status: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Nv0080Alloc {
    device_id: u32,
    h_client_share: u32,
    h_target_client: u32,
    h_target_device: u32,
    flags: u32,
    va_space_size: u64,
    va_start_internal: u64,
    va_limit_internal: u64,
    va_mode: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Nv2080Alloc {
    sub_device_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ClkDomainInfo {
    obj_type: u8,
    obj_reserved: [u8; 3],
    domain_bit: u32,
    tail: [u8; 0x178],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ClkDomainsInfo {
    obj_mask: u32,
    domain_mask: u32,
    reserved: [u8; 0x28],
    domains: [ClkDomainInfo; CLK_DOMAIN_COUNT],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ClkDomainControl {
    obj_type: u32,
    reserved_1: u32,
    freq_offset_mode: u8,
    reserved_2: [u8; 3],
    freq_offset_khz: i32,
    voltage_offsets_uv: [i32; 4],
    reserved_3: [u8; 0x20],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ClkDomainsControl {
    obj_mask: u32,
    domain_mask: u32,
    reserved: [u8; 0x34],
    domains: [ClkDomainControl; CLK_DOMAIN_COUNT],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ClkMeasureFreq {
    domain_mask: u32,
    freq_khz: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ClkPropRelInfo {
    rel_type: u8,
    reserved_1: u8,
    source_index: u8,
    dest_index: u8,
    bidirectional: u8,
    reserved_2: [u8; 7],
    ratio: u32,
    inverse_ratio: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ClkPropRelsInfo {
    obj_mask: u32,
    relation_mask: u32,
    reserved: [u8; 0x120],
    relations: [ClkPropRelInfo; CLK_PROP_REL_COUNT],
    tail: [u8; 0x1170],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ClkPropRelControl {
    rel_type: u8,
    reserved_1: [u8; 7],
    ratio: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ClkPropRelsControl {
    header: [u8; 0x24],
    relations: [ClkPropRelControl; CLK_PROP_REL_COUNT],
    tail: [u8; 0xa74],
}

const _: () = {
    assert!(mem::size_of::<NvOs21>() == 32);
    assert!(mem::size_of::<NvOs54>() == 32);
    assert!(mem::size_of::<NvOs64>() == 48);
    assert!(mem::size_of::<Nv0080Alloc>() == 56);
    assert!(mem::size_of::<Nv2080Alloc>() == 4);
    assert!(mem::size_of::<ClkDomainInfo>() == 0x180);
    assert!(mem::size_of::<ClkDomainsInfo>() == 0x3030);
    assert!(mem::size_of::<ClkDomainControl>() == 0x40);
    assert!(mem::size_of::<ClkDomainsControl>() == 0x83c);
    assert!(mem::size_of::<ClkMeasureFreq>() == 8);
    assert!(mem::size_of::<ClkPropRelInfo>() == 0x14);
    assert!(mem::size_of::<ClkPropRelsInfo>() == 0x1518);
    assert!(mem::size_of::<ClkPropRelControl>() == 0xc);
    assert!(mem::size_of::<ClkPropRelsControl>() == 0xc18);
};

ioctl_readwrite!(rm_alloc21, NV_IOCTL_MAGIC, NV_ESC_RM_ALLOC, NvOs21);
ioctl_readwrite!(rm_alloc64, NV_IOCTL_MAGIC, NV_ESC_RM_ALLOC, NvOs64);
ioctl_readwrite!(rm_control, NV_IOCTL_MAGIC, NV_ESC_RM_CONTROL, NvOs54);
ioctl_readwrite!(register_fd, NV_IOCTL_MAGIC, NV_ESC_REGISTER_FD, i32);

fn check(status: u32, what: &str) -> Result<(), String> {
    if status != 0 {
        return Err(format!("{what} failed with status {status:#x}"));
    }
    Ok(())
}

fn entry_range(entry: &ClkDomainInfo) -> Option<(i32, i32)> {
    let offset = match entry.obj_type {
        TYPE_35 => RANGE_OFFSET_35,
        TYPE_3X => RANGE_OFFSET_3X,
        _ => return None,
    };
    let at = offset - mem::offset_of!(ClkDomainInfo, tail);
    let min = i16::from_le_bytes(entry.tail[at..at + 2].try_into().unwrap());
    let max = i32::from_le_bytes(entry.tail[at + 2..at + 6].try_into().unwrap());
    (max > 0).then_some((i32::from(min), max))
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn measure(fd: RawFd, client: u32, subdevice: u32, domain_id: u32) -> Result<u32, String> {
    let mut params = ClkMeasureFreq {
        domain_mask: 1 << domain_id,
        freq_khz: 0,
    };
    unsafe {
        let mut request = NvOs54 {
            h_client: client,
            h_object: subdevice,
            cmd: CMD_CLK_MEASURE_FREQ,
            flags: 0,
            params: &raw mut params as u64,
            params_size: 8,
            status: 0,
        };
        rm_control(fd, &raw mut request).map_err(|err| err.to_string())?;
        check(request.status, "CLK_MEASURE_FREQ")?;
    }
    Ok(params.freq_khz)
}

fn control_domain(fd: RawFd, client: u32, subdevice: u32, mask: u32) -> Result<ClkDomainsControl, String> {
    let mut control = ClkDomainsControl {
        obj_mask: 0,
        domain_mask: mask,
        reserved: [0; 0x34],
        domains: unsafe { mem::zeroed() },
    };
    unsafe {
        let mut request = NvOs54 {
            h_client: client,
            h_object: subdevice,
            cmd: CMD_CLK_CLK_DOMAINS_GET_CONTROL,
            flags: 0,
            params: &raw mut control as u64,
            params_size: mem::size_of::<ClkDomainsControl>() as u32,
            status: 0,
        };
        rm_control(fd, &raw mut request).map_err(|err| err.to_string())?;
        check(request.status, "CLK_CLK_DOMAINS_GET_CONTROL")?;
    }
    Ok(control)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let write_test: Option<(u32, i32)> = match args.as_slice() {
        [] => None,
        [flag, domain, microvolts] if flag == "--write-test" => Some((
            domain.parse().expect("domain id"),
            microvolts.parse().expect("microvolts"),
        )),
        _ => panic!("usage: clkdom_probe [--write-test <domain-id> <microvolts>]"),
    };

    if !geteuid().is_root() {
        println!("note: not root — RM controls answer this deep with status 0x1b");
    }

    let nvidiactl: OwnedFd = File::options()
        .read(true)
        .write(true)
        .open("/dev/nvidiactl")
        .expect("open /dev/nvidiactl")
        .into();
    let device_fd: OwnedFd = File::options()
        .read(true)
        .write(true)
        .open("/dev/nvidia0")
        .expect("open /dev/nvidia0")
        .into();

    let fd = nvidiactl.as_raw_fd();
    unsafe {
        let mut device_raw = device_fd.as_raw_fd();
        register_fd(fd, &raw mut device_raw).expect("REGISTER_FD");
    }

    let client = unsafe {
        let mut request: NvOs21 = mem::zeroed();
        rm_alloc21(fd, &raw mut request).expect("RM_ALLOC client");
        check(request.status, "RM_ALLOC client").unwrap();
        request.h_object_new
    };

    let device = unsafe {
        let alloc = Nv0080Alloc {
            device_id: 0,
            h_client_share: 0,
            h_target_client: 0,
            h_target_device: 0,
            flags: 0,
            va_space_size: 0,
            va_start_internal: 0,
            va_limit_internal: 0,
            va_mode: 0,
        };
        let mut request = NvOs64 {
            h_root: client,
            h_object_parent: client,
            h_object_new: 0,
            h_class: NV01_DEVICE_0,
            p_alloc_parms: &raw const alloc as u64,
            p_rights_requested: 0,
            params_size: mem::size_of::<Nv0080Alloc>() as u32,
            flags: 0,
            status: 0,
        };
        rm_alloc64(fd, &raw mut request).expect("RM_ALLOC device");
        check(request.status, "RM_ALLOC device").unwrap();
        request.h_object_new
    };

    let subdevice = unsafe {
        let alloc = Nv2080Alloc { sub_device_id: 0 };
        let mut request = NvOs64 {
            h_root: client,
            h_object_parent: device,
            h_object_new: 0,
            h_class: NV20_SUBDEVICE_0,
            p_alloc_parms: &raw const alloc as u64,
            p_rights_requested: 0,
            params_size: mem::size_of::<Nv2080Alloc>() as u64 as u32,
            flags: 0,
            status: 0,
        };
        rm_alloc64(fd, &raw mut request).expect("RM_ALLOC subdevice");
        check(request.status, "RM_ALLOC subdevice").unwrap();
        request.h_object_new
    };

    let info = {
        let mut info: ClkDomainsInfo = unsafe { mem::zeroed() };
        unsafe {
            let mut request = NvOs54 {
                h_client: client,
                h_object: subdevice,
                cmd: CMD_CLK_CLK_DOMAINS_GET_INFO,
                flags: 0,
                params: &raw mut info as u64,
                params_size: mem::size_of::<ClkDomainsInfo>() as u32,
                status: 0,
            };
            rm_control(fd, &raw mut request).expect("CLK_CLK_DOMAINS_GET_INFO");
            check(request.status, "CLK_CLK_DOMAINS_GET_INFO").unwrap();
        }
        info
    };

    let control = control_domain(fd, client, subdevice, info.domain_mask).unwrap();

    println!("GET_INFO domain_mask: {:#010x}", info.domain_mask);
    println!("GET_CONTROL obj_mask:   {:#010x}", control.obj_mask);
    println!();
    println!("idx type bit  name    range(MHz)   freq_off   rail0-3 uV (rail1=MSVDD)");
    for index in 0..CLK_DOMAIN_COUNT {
        if info.domain_mask & (1 << index) == 0 {
            continue;
        }
        let entry = &info.domains[index];
        if !entry.domain_bit.is_power_of_two() {
            continue;
        }
        let id = entry.domain_bit.trailing_zeros();
        let range = entry_range(entry)
            .map(|(min, max)| format!("{min:+}..{max:+}"))
            .unwrap_or_else(|| "none".into());
        let control_entry = &control.domains[index];
        println!(
            "{index:3} {:02x}  {:#04x} {:6} {:11} {:8} kHz   [{}, {}, {}, {}]",
            entry.obj_type,
            entry.domain_bit,
            domain_name(id),
            range,
            control_entry.freq_offset_khz,
            control_entry.voltage_offsets_uv[0],
            control_entry.voltage_offsets_uv[1],
            control_entry.voltage_offsets_uv[2],
            control_entry.voltage_offsets_uv[3],
        );
        println!("      audit raw at both range offsets: [0x26] {} | [0x22] {}",
            hex(&entry.tail[0x26 - mem::offset_of!(ClkDomainInfo, tail)..0x26 - mem::offset_of!(ClkDomainInfo, tail) + 8]),
            hex(&entry.tail[0x22 - mem::offset_of!(ClkDomainInfo, tail)..0x22 - mem::offset_of!(ClkDomainInfo, tail) + 8]),
        );
    }

    println!();
    println!("CLK_MEASURE_FREQ (physical, kHz, at whatever load exists now):");
    for index in 0..CLK_DOMAIN_COUNT {
        if info.domain_mask & (1 << index) == 0 {
            continue;
        }
        let id = info.domains[index].domain_bit.trailing_zeros();
        let measured = measure(fd, client, subdevice, id)
            .map(|khz| format!("{khz}"))
            .unwrap_or_else(|err| err);
        println!("  domain {:2} {:6}: {}", id, domain_name(id), measured);
    }

    let rels_info = unsafe {
        let mut info: ClkPropRelsInfo = mem::zeroed();
        let mut request = NvOs54 {
            h_client: client,
            h_object: subdevice,
            cmd: CMD_CLK_TOP_PROP_RELS_GET_INFO,
            flags: 0,
            params: &raw mut info as u64,
            params_size: mem::size_of::<ClkPropRelsInfo>() as u32,
            status: 0,
        };
        rm_control(fd, &raw mut request).expect("CLK_TOP_PROP_RELS_GET_INFO");
        check(request.status, "CLK_TOP_PROP_RELS_GET_INFO").unwrap();
        info
    };

    let mut rels_control: ClkPropRelsControl = unsafe { mem::zeroed() };
    rels_control.header.copy_from_slice(unsafe {
        slice_from(&rels_info as *const ClkPropRelsInfo as *const u8, 0x24)
    });
    unsafe {
        let mut request = NvOs54 {
            h_client: client,
            h_object: subdevice,
            cmd: CMD_CLK_TOP_PROP_RELS_GET_CONTROL,
            flags: 0,
            params: &raw mut rels_control as u64,
            params_size: mem::size_of::<ClkPropRelsControl>() as u32,
            status: 0,
        };
        rm_control(fd, &raw mut request).expect("CLK_TOP_PROP_RELS_GET_CONTROL");
        check(request.status, "CLK_TOP_PROP_RELS_GET_CONTROL").unwrap();
    }

    println!();
    println!("PROP_RELS relation_mask: {:#010x}", rels_info.relation_mask);
    for index in 0..CLK_PROP_REL_COUNT {
        if rels_info.relation_mask & (1 << index) == 0 {
            continue;
        }
        let relation = &rels_info.relations[index];
        let name = |i| domain_name(info.domains[i as usize].domain_bit.trailing_zeros());
        println!(
            "  rel {index:2}: type {} src {}({}) dest {}({}) bidir {} factory ratio {:#x} ({:.6}) control ratio {:#x} ({:.6})",
            relation.rel_type,
            relation.source_index,
            name(relation.source_index),
            relation.dest_index,
            name(relation.dest_index),
            relation.bidirectional,
            relation.ratio,
            relation.ratio as f64 / 65536.0,
            rels_control.relations[index].ratio,
            rels_control.relations[index].ratio as f64 / 65536.0,
        );
    }

    if let Some((domain_id, microvolts)) = write_test {
        let index = (0..CLK_DOMAIN_COUNT)
            .filter(|index| info.domain_mask & (1 << index) != 0)
            .find(|index| info.domains[*index].domain_bit == 1 << domain_id)
            .unwrap_or_else(|| panic!("GPU has no clock domain {domain_id}"));

        let preimage = control_domain(fd, client, subdevice, info.domain_mask).unwrap();
        println!();
        println!("write test: domain {domain_id}, rail {MSVDD_RAIL_INDEX}, {microvolts} uV");
        println!("  preimage rail: {} uV", preimage.domains[index].voltage_offsets_uv[MSVDD_RAIL_INDEX]);

        let mut write = preimage;
        write.domains[index].voltage_offsets_uv[MSVDD_RAIL_INDEX] = microvolts;

        unsafe {
            let mut request = NvOs54 {
                h_client: client,
                h_object: subdevice,
                cmd: CMD_CLK_CLK_DOMAINS_SET_CONTROL,
                flags: 0,
                params: &raw mut write as u64,
                params_size: mem::size_of::<ClkDomainsControl>() as u32,
                status: 0,
            };
            rm_control(fd, &raw mut request).expect("SET_CONTROL");
            match request.status {
                0 => println!("  SET: status 0"),
                other => {
                    println!("  SET: status {other:#x} — rail field rejected on this branch");
                    return;
                }
            }
        }

        let readback = control_domain(fd, client, subdevice, info.domain_mask).unwrap();
        println!("  readback rail: {} uV", readback.domains[index].voltage_offsets_uv[MSVDD_RAIL_INDEX]);
        println!("  measured after: {} kHz", measure(fd, client, subdevice, domain_id).unwrap_or_default());

        let mut restore = preimage;
        unsafe {
            let mut request = NvOs54 {
                h_client: client,
                h_object: subdevice,
                cmd: CMD_CLK_CLK_DOMAINS_SET_CONTROL,
                flags: 0,
                params: &raw mut restore as u64,
                params_size: mem::size_of::<ClkDomainsControl>() as u32,
                status: 0,
            };
            rm_control(fd, &raw mut request).expect("SET_CONTROL restore");
            println!("  restore: status {:#x}", request.status);
        }
        let restored = control_domain(fd, client, subdevice, info.domain_mask).unwrap();
        let exact = bytematch(&restored, &preimage);
        println!("  restore readback exact: {exact}");
        println!("  measured restored: {} kHz", measure(fd, client, subdevice, domain_id).unwrap_or_default());
    }
}

fn bytematch(a: &ClkDomainsControl, b: &ClkDomainsControl) -> bool {
    let a = unsafe { slice_from(a as *const _ as *const u8, mem::size_of::<ClkDomainsControl>()) };
    let b = unsafe { slice_from(b as *const _ as *const u8, mem::size_of::<ClkDomainsControl>()) };
    a == b
}

unsafe fn slice_from<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    unsafe { std::slice::from_raw_parts(ptr, len) }
}
