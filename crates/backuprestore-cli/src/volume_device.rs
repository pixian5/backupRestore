//! 底层卷与物理存储设备探测器（Win32 Native Volume & Storage Device）
//!
//! 负责：
//! - Win32 卷与磁盘设备底层调用（`CreateFileW`, `DeviceIoControl`, `GetVolumeInformationW`, `GetDiskFreeSpaceExW` 等）；
//! - 卷身份物理特征提取（`VolumeIdentity`，分区 GUID/偏移/大小/文件系统/序列号）；
//! - 免盘符卷路径（Verbatim Volume Path）与设备路径物理身份解析；
//! - 驱动器可用与剩余空间定量获取；
//! - 卷缓冲区强刷（`FlushFileBuffers`）。

#[cfg(windows)]
use std::env;
#[cfg(windows)]
use std::ffi::c_void;
#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use std::ptr::null_mut;

#[cfg(windows)]
use backuprestore_core::{TaskError, VolumeIdentity};
#[cfg(windows)]
use crate::error_code::ErrCode;
#[cfg(windows)]
use crate::{append_log, err};

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetDiskFreeSpaceExW(
        directory_name: *const u16,
        free_bytes_available: *mut u64,
        total_number_of_bytes: *mut u64,
        total_number_of_free_bytes: *mut u64,
    ) -> i32;
    fn GetVolumeInformationW(
        root_path_name: *const u16,
        volume_name_buffer: *mut u16,
        volume_name_size: u32,
        volume_serial_number: *mut u32,
        maximum_component_length: *mut u32,
        file_system_flags: *mut u32,
        file_system_name_buffer: *mut u16,
        file_system_name_size: u32,
    ) -> i32;
    fn GetVolumeNameForVolumeMountPointW(
        volume_mount_point: *const u16,
        volume_name: *mut u16,
        buffer_length: u32,
    ) -> i32;
    fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *mut c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: *mut c_void,
    ) -> *mut c_void;
    fn DeviceIoControl(
        device: *mut c_void,
        control_code: u32,
        input_buffer: *mut c_void,
        input_size: u32,
        output_buffer: *mut c_void,
        output_size: u32,
        bytes_returned: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn FlushFileBuffers(handle: *mut c_void) -> i32;
    fn GetLastError() -> u32;
}

#[cfg(windows)]
const GENERIC_READ: u32 = 0x8000_0000;
#[cfg(windows)]
const GENERIC_WRITE: u32 = 0x4000_0000;
#[cfg(windows)]
const FILE_SHARE_READ: u32 = 0x0000_0001;
#[cfg(windows)]
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
#[cfg(windows)]
const FILE_SHARE_DELETE: u32 = 0x0000_0004;
#[cfg(windows)]
const OPEN_EXISTING: u32 = 3;
#[cfg(windows)]
const IOCTL_DISK_GET_PARTITION_INFO_EX: u32 = 0x0007_0048;
#[cfg(windows)]
const IOCTL_DISK_GET_DRIVE_LAYOUT_EX: u32 = 0x0007_0050;
#[cfg(windows)]
const IOCTL_STORAGE_GET_DEVICE_NUMBER: u32 = 0x002d_1080;

#[cfg(windows)]
pub(crate) struct PhysicalVolumeIdentity {
    pub disk_guid: String,
    pub partition_guid: String,
    pub partition_type_guid: String,
    pub partition_number: u32,
    pub partition_offset: u64,
    pub partition_size: u64,
}

#[cfg(windows)]
pub(crate) fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
pub(crate) fn open_device(path: &[u16]) -> Result<*mut c_void, TaskError> {
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null_mut(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    if handle as isize == -1 {
        let error_code = unsafe { GetLastError() };
        let shown = String::from_utf16_lossy(path)
            .trim_matches(char::from(0))
            .to_string();
        Err(err(&format!(
            "CreateFileW failed on {shown} while reading volume identity (Windows error {error_code})"
        )))
    } else {
        Ok(handle)
    }
}

#[cfg(windows)]
pub(crate) fn device_io_control(
    handle: *mut c_void,
    control_code: u32,
    output: &mut [u8],
) -> Result<(), TaskError> {
    let mut returned = 0_u32;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            control_code,
            null_mut(),
            0,
            output.as_mut_ptr() as *mut c_void,
            output.len() as u32,
            &mut returned,
            null_mut(),
        )
    };
    if ok == 0 {
        Err(err("DeviceIoControl failed while reading GPT identity"))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
pub(crate) fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, TaskError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| err("buffer too short for DWORD"))?;
    Ok(u32::from_le_bytes(slice.try_into().map_err(|_| {
        err("failed to convert slice to DWORD")
    })?))
}

#[cfg(windows)]
pub(crate) fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, TaskError> {
    let slice = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| err("buffer too short for QWORD"))?;
    Ok(u64::from_le_bytes(slice.try_into().map_err(|_| {
        err("failed to convert slice to QWORD")
    })?))
}

#[cfg(windows)]
pub(crate) fn format_guid(bytes: &[u8]) -> Result<String, TaskError> {
    if bytes.len() < 16 {
        return Err(err("buffer too short for GUID"));
    }
    let data1 = u32::from_le_bytes(
        bytes[0..4]
            .try_into()
            .map_err(|_| err("failed to decode GUID data1"))?,
    );
    let data2 = u16::from_le_bytes(
        bytes[4..6]
            .try_into()
            .map_err(|_| err("failed to decode GUID data2"))?,
    );
    let data3 = u16::from_le_bytes(
        bytes[6..8]
            .try_into()
            .map_err(|_| err("failed to decode GUID data3"))?,
    );
    Ok(format!(
        "{{{data1:08x}-{data2:04x}-{data3:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}",
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    ))
}

#[cfg(windows)]
pub(crate) fn volume_information_at(path: &str) -> Result<(String, String), TaskError> {
    let path = wide_null(path);
    let mut volume_name = vec![0_u16; 256];
    let mut filesystem = vec![0_u16; 64];
    let mut serial = 0_u32;
    let mut maximum_component_length = 0_u32;
    let mut flags = 0_u32;
    let ok = unsafe {
        GetVolumeInformationW(
            path.as_ptr(),
            volume_name.as_mut_ptr(),
            volume_name.len() as u32,
            &mut serial,
            &mut maximum_component_length,
            &mut flags,
            filesystem.as_mut_ptr(),
            filesystem.len() as u32,
        )
    };
    if ok == 0 {
        let code = unsafe { GetLastError() };
        let shown = String::from_utf16_lossy(&path);
        return Err(err(&format!(
            "GetVolumeInformationW failed for {shown} with Win32 error {code}"
        )));
    }
    let length = filesystem
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(filesystem.len());
    Ok((
        String::from_utf16_lossy(&filesystem[..length]),
        format!("{serial:08X}"),
    ))
}

#[cfg(windows)]
pub(crate) fn volume_information(letter: char) -> Result<(String, String), TaskError> {
    volume_information_at(&format!(r"{letter}:\\"))
}

#[cfg(windows)]
pub(crate) fn storage_device_number_at(path: &str) -> Result<u32, TaskError> {
    let handle = open_device(&wide_null(path))?;
    let mut number = [0_u8; 12];
    let result = device_io_control(handle, IOCTL_STORAGE_GET_DEVICE_NUMBER, &mut number)
        .and_then(|_| read_u32(&number, 4));
    unsafe { CloseHandle(handle) };
    result
}

#[cfg(windows)]
pub(crate) fn storage_device_number(letter: char) -> Result<u32, TaskError> {
    storage_device_number_at(&format!(r"\\.\{letter}:"))
}

#[cfg(windows)]
pub(crate) fn physical_volume_identity_at(
    path: &str,
    disk_number: u32,
) -> Result<PhysicalVolumeIdentity, TaskError> {
    let partition_handle = open_device(&wide_null(path))?;
    let mut partition = vec![0_u8; 160];
    let partition_query = device_io_control(
        partition_handle,
        IOCTL_DISK_GET_PARTITION_INFO_EX,
        &mut partition,
    )
    .and_then(|_| {
        let partition_offset = read_u64(&partition, 8)?;
        let partition_size = read_u64(&partition, 16)?;
        let partition_number = read_u32(&partition, 24)?;
        let partition_type_guid = format_guid(&partition[32..48])?;
        let partition_guid = format_guid(&partition[48..64])?;
        Ok((
            partition_offset,
            partition_size,
            partition_number,
            partition_type_guid,
            partition_guid,
        ))
    });
    let _ = unsafe { CloseHandle(partition_handle) };
    let (partition_offset, partition_size, partition_number, partition_type_guid, partition_guid) =
        partition_query?;
    let disk_path = format!(r"\\.\PhysicalDrive{}", disk_number);
    let disk_handle = open_device(&wide_null(&disk_path))?;
    let mut layout = vec![0_u8; 65_536];
    let layout_query = device_io_control(disk_handle, IOCTL_DISK_GET_DRIVE_LAYOUT_EX, &mut layout)
        .and_then(|_| {
            if read_u32(&layout, 0)? != 1 {
                return Err(err("selected disk is not GPT"));
            }
            format_guid(&layout[8..24])
        });
    let _ = unsafe { CloseHandle(disk_handle) };
    let disk_guid = layout_query?;
    Ok(PhysicalVolumeIdentity {
        disk_guid,
        partition_guid,
        partition_type_guid,
        partition_number,
        partition_offset,
        partition_size,
    })
}

#[cfg(windows)]
pub(crate) fn physical_volume_identity(
    letter: char,
    disk_number: u32,
) -> Result<PhysicalVolumeIdentity, TaskError> {
    physical_volume_identity_at(&format!(r"\\.\{letter}:"), disk_number)
}

#[cfg(windows)]
pub(crate) fn volume_identity_at_path(
    path: &str,
    volume_guid: String,
) -> Result<VolumeIdentity, TaskError> {
    let device_path = crate::text_parsing::volume_path_to_device_path(path)
        .ok_or_else(|| err("volume path has no GUID; cannot derive its device path"))?;
    let (filesystem, volume_serial) = volume_information_at(path)?;
    let disk_number = storage_device_number_at(&device_path)?;
    let physical = physical_volume_identity_at(&device_path, disk_number)?;
    Ok(VolumeIdentity {
        disk_guid: physical.disk_guid,
        partition_guid: physical.partition_guid,
        volume_guid,
        partition_type_guid: physical.partition_type_guid,
        disk_number: Some(disk_number),
        partition_number: Some(physical.partition_number),
        partition_offset: physical.partition_offset,
        partition_size: physical.partition_size,
        filesystem,
        volume_serial,
        drive_letter: None,
    })
}

#[cfg(windows)]
pub(crate) fn volume_identity(letter: char) -> Result<VolumeIdentity, TaskError> {
    let volume_guid = match mounted_volume_guid(letter) {
        Ok(Some(volume_guid)) => volume_guid,
        Ok(None) => return Err(err(&format!("no volume is mounted at {letter}:"))),
        Err(error) => {
            return Err(err(&format!(
                "the volume API returned error {error} for {letter}:"
            )))
        }
    };
    let (filesystem, volume_serial) = volume_information(letter)?;
    let disk_number = storage_device_number(letter)?;
    let physical = physical_volume_identity(letter, disk_number)?;
    Ok(VolumeIdentity {
        disk_guid: physical.disk_guid,
        partition_guid: physical.partition_guid,
        volume_guid,
        partition_type_guid: physical.partition_type_guid,
        disk_number: Some(disk_number),
        partition_number: Some(physical.partition_number),
        partition_offset: physical.partition_offset,
        partition_size: physical.partition_size,
        filesystem,
        volume_serial,
        drive_letter: Some(letter),
    })
}

#[cfg(windows)]
pub(crate) fn volume_identity_with_known_guid(
    letter: char,
    volume_guid: String,
) -> Result<VolumeIdentity, TaskError> {
    let (filesystem, volume_serial) = volume_information(letter)?;
    let disk_number = storage_device_number(letter)?;
    let physical = physical_volume_identity(letter, disk_number)?;
    Ok(VolumeIdentity {
        disk_guid: physical.disk_guid,
        partition_guid: physical.partition_guid,
        volume_guid,
        partition_type_guid: physical.partition_type_guid,
        disk_number: Some(disk_number),
        partition_number: Some(physical.partition_number),
        partition_offset: physical.partition_offset,
        partition_size: physical.partition_size,
        filesystem,
        volume_serial,
        drive_letter: Some(letter),
    })
}

#[cfg(windows)]
pub(crate) fn active_system_volume_identity() -> Result<VolumeIdentity, TaskError> {
    let value = env::var("SystemDrive").map_err(|_| crate::errc(ErrCode::CurrentSystemVolumeUnknown))?;
    let letter = value
        .trim()
        .chars()
        .next()
        .ok_or_else(|| crate::errc(ErrCode::CurrentSystemVolumeUnknown))?
        .to_ascii_uppercase();
    volume_identity(letter)
}

#[cfg(windows)]
pub(crate) fn mounted_volume_guid(letter: char) -> Result<Option<String>, u32> {
    let root = wide_null(&format!(r"{}:\", letter));
    let mut volume = [0_u16; 128];
    let success = unsafe {
        GetVolumeNameForVolumeMountPointW(root.as_ptr(), volume.as_mut_ptr(), volume.len() as u32)
    };
    if success == 0 {
        let error = unsafe { GetLastError() };
        if error == 3 || error == 2 {
            return Ok(None);
        }
        return Err(error);
    }
    let Some(length) = volume.iter().position(|value| *value == 0) else {
        return Err(122);
    };
    Ok(Some(String::from_utf16_lossy(&volume[..length])))
}

#[cfg(windows)]
pub(crate) fn volume_free_bytes(letter: char) -> Result<u64, TaskError> {
    disk_free_space(letter).map(|(_, free)| free)
}

#[cfg(windows)]
pub(crate) fn disk_free_space(letter: char) -> Result<(u64, u64), TaskError> {
    let path = wide_null(&format!(r"{}:\", letter));
    let mut available = 0_u64;
    let mut total = 0_u64;
    let mut free = 0_u64;
    let ok = unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut available, &mut total, &mut free) };
    if ok == 0 {
        return Err(err(&format!("GetDiskFreeSpaceExW failed for {letter}:")));
    }
    Ok((total, available.min(free)))
}

#[cfg(windows)]
pub(crate) fn partition_of_device_path(device: &str) -> Result<(u32, u32), TaskError> {
    let name = crate::text_parsing::device_path_volume_name(device)
        .ok_or_else(|| crate::errc_with(ErrCode::DevicePathUnrecognized, &[device]))?;
    let path = format!(r"\\.\{name}");
    let disk = storage_device_number_at(&path)?;
    let physical = physical_volume_identity_at(&path, disk)?;
    Ok((disk, physical.partition_number))
}

#[cfg(windows)]
pub(crate) fn flush_volume(letter: char, label: &str, log: &Path) -> Result<(), TaskError> {
    let path: Vec<u16> = format!(r"\\.\\{}:", letter.to_ascii_uppercase())
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null_mut(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    if handle as isize == -1 {
        return Err(crate::errc_with(
            ErrCode::VolumeFlushOpenFailed,
            &[&letter.to_string(), label, &unsafe { GetLastError() }.to_string()],
        ));
    }
    let ok = unsafe { FlushFileBuffers(handle) };
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return Err(crate::errc_with(
            ErrCode::VolumeFlushFailed,
            &[&letter.to_string(), label, &unsafe { GetLastError() }.to_string()],
        ));
    }
    append_log(log, &crate::i18n::msg_tpl("wp_volume_cache_flushed", &[&format!("{letter}"), label]))?;
    Ok(())
}
