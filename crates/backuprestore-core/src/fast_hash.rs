//! 文件 SHA-256 的快速路径：Windows 上改用系统自带的加密接口（`bcrypt.dll`）。
//!
//! # 为什么要有这个模块（2026-11-24 立项）
//!
//! [`crate::sha256_file`] 原先只用纯软件实现的 `sha2`。它本身不慢——完整 Windows 里
//! 实测约 490 MB/秒；但同一份程序在恢复环境里给一份 40 GB 的镜像算哈希要 12 分钟，
//! 备份的「校验」一步因此比捕获还久。系统自带的加密接口会用处理器的硬件加速，
//! 同一台机器上系统工具算同一个文件不到 60 秒。
//!
//! # 三条设计约束
//!
//! 1. **结果必须与软件实现逐位相同。** 两条路径算的是同一个 SHA-256，单测用多种
//!    大小的文件逐字节比对；任何一边算错都会在单测里暴露。
//! 2. **接口在运行时动态加载，不在启动时静态链接。** 这个函数在恢复环境里也要跑；
//!    如果某个精简过的自定义恢复环境把 `bcrypt.dll` 裁掉了，静态链接会让程序
//!    **直接起不来**（与漏掉静态运行库同一个症状：`0xC0000135`、零输出零副作用），
//!    而动态加载只是退回软件实现。
//! 3. **接口不可用就回落，不在失败处报错。** 回落的代价只是慢，结果不变。
//!
//! 非 Windows 平台没有这个模块，[`crate::sha256_file`] 只走软件实现。

/// 当前平台上**可用**的哈希实现：Windows 且 `bcrypt.dll` 加载成功为 `"bcrypt"`，
/// 否则为 `"software"`。只用于单测与诊断；要知道某一次调用实际用了哪种，
/// 用 [`crate::task_store::sha256_file_with_backend`]（接口中途报错会回落，两者可能不同）。
pub fn sha256_backend() -> &'static str {
    if cfg!(windows) && windows::bcrypt_available() {
        "bcrypt"
    } else {
        "software"
    }
}

#[cfg(windows)]
pub(crate) mod windows {
    use std::ffi::c_void;
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use std::sync::OnceLock;

    use crate::TaskError;
    use crate::task_store::read_file_chunks;

    type Handle = *mut c_void;
    /// NTSTATUS：成功为 0，失败时最高位为 1（按有符号整数看是负数）。
    type Status = i32;

    type FnOpen = unsafe extern "system" fn(*mut Handle, *const u16, *const u16, u32) -> Status;
    type FnClose = unsafe extern "system" fn(Handle, u32) -> Status;
    type FnGetProperty =
        unsafe extern "system" fn(Handle, *const u16, *mut u8, u32, *mut u32, u32) -> Status;
    type FnCreateHash =
        unsafe extern "system" fn(Handle, *mut Handle, *mut u8, u32, *const u8, u32, u32) -> Status;
    type FnHashData = unsafe extern "system" fn(Handle, *const u8, u32, u32) -> Status;
    type FnFinishHash = unsafe extern "system" fn(Handle, *mut u8, u32, u32) -> Status;
    type FnDestroyHash = unsafe extern "system" fn(Handle) -> Status;

    /// 从 `bcrypt.dll` 取出的七个函数。模块句柄故意不释放：进程存活期间一直要用，
    /// 释放后这些函数指针就成了悬垂指针。句柄按整数保存，这样整个结构体才是 `Sync`。
    struct Bcrypt {
        _module: usize,
        open: FnOpen,
        close: FnClose,
        get_property: FnGetProperty,
        create_hash: FnCreateHash,
        hash_data: FnHashData,
        finish_hash: FnFinishHash,
        destroy_hash: FnDestroyHash,
    }

    static BCRYPT: OnceLock<Option<Bcrypt>> = OnceLock::new();

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(file_name: *const u16) -> Handle;
        fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 只在第一次调用时加载；之后返回同一份结果。任何一步失败都记为「不可用」，
    /// 进程内不再重试（恢复环境里座行为不会中途变化，重试只是白费工夫）。
    fn load() -> Option<Bcrypt> {
        // SAFETY: 只调用 LoadLibraryW / GetProcAddress 这两个系统接口；传入的字符串
        // 都以 0 结尾，模块句柄在进程存活期间保持有效。
        unsafe {
            let name = wide("bcrypt.dll");
            let module = LoadLibraryW(name.as_ptr());
            if module.is_null() {
                return None;
            }
            let symbol = |name: &[u8]| GetProcAddress(module, name.as_ptr());
            let open = symbol(b"BCryptOpenAlgorithmProvider\0");
            let close = symbol(b"BCryptCloseAlgorithmProvider\0");
            let get_property = symbol(b"BCryptGetProperty\0");
            let create_hash = symbol(b"BCryptCreateHash\0");
            let hash_data = symbol(b"BCryptHashData\0");
            let finish_hash = symbol(b"BCryptFinishHash\0");
            let destroy_hash = symbol(b"BCryptDestroyHash\0");
            if open.is_null()
                || close.is_null()
                || get_property.is_null()
                || create_hash.is_null()
                || hash_data.is_null()
                || finish_hash.is_null()
                || destroy_hash.is_null()
            {
                return None;
            }
            // SAFETY: 这些地址来自 bcrypt.dll 的导出表，签名按系统头文件照抄。
            Some(Bcrypt {
                _module: module as usize,
                open: std::mem::transmute::<*mut c_void, FnOpen>(open),
                close: std::mem::transmute::<*mut c_void, FnClose>(close),
                get_property: std::mem::transmute::<*mut c_void, FnGetProperty>(get_property),
                create_hash: std::mem::transmute::<*mut c_void, FnCreateHash>(create_hash),
                hash_data: std::mem::transmute::<*mut c_void, FnHashData>(hash_data),
                finish_hash: std::mem::transmute::<*mut c_void, FnFinishHash>(finish_hash),
                destroy_hash: std::mem::transmute::<*mut c_void, FnDestroyHash>(destroy_hash),
            })
        }
    }

    pub(crate) fn bcrypt_available() -> bool {
        BCRYPT.get_or_init(load).is_some()
    }

    /// 算法提供者句柄的守卫：离开作用域时关闭，无论中途成功还是出错。
    struct Algorithm<'a> {
        bcrypt: &'a Bcrypt,
        handle: Handle,
    }

    impl Drop for Algorithm<'_> {
        fn drop(&mut self) {
            // SAFETY: 句柄由 BCryptOpenAlgorithmProvider 成功返回，只关闭一次。
            unsafe {
                (self.bcrypt.close)(self.handle, 0);
            }
        }
    }

    /// 哈希句柄的守卫：离开作用域时销毁。
    struct Hash<'a> {
        bcrypt: &'a Bcrypt,
        handle: Handle,
    }

    impl Drop for Hash<'_> {
        fn drop(&mut self) {
            // SAFETY: 句柄由 BCryptCreateHash 成功返回，只销毁一次。
            unsafe {
                (self.bcrypt.destroy_hash)(self.handle);
            }
        }
    }

    /// 用系统加密接口给文件算 SHA-256。
    ///
    /// - `Ok(Some(十六进制摘要))`：算出来了。
    /// - `Ok(None)`：接口不可用，或接口中途报错——两种情况调用方都回落到软件实现
    ///   （结果相同，只是慢）。文件本身读不了的错误原样上抛，不在这里吞掉。
    pub(crate) fn sha256_file(path: &Path) -> Result<Option<String>, TaskError> {
        let Some(bcrypt) = BCRYPT.get_or_init(load).as_ref() else {
            return Ok(None);
        };
        // SAFETY: 以下对 bcrypt 的调用都按系统文档的参数约定；所有输出缓冲区都由本函数
        // 持有足够长的生命周期，字符串参数都以 0 结尾。
        unsafe {
            let algorithm_name = wide("SHA256");
            let mut algorithm: Handle = null_mut();
            if (bcrypt.open)(&mut algorithm, algorithm_name.as_ptr(), null(), 0) < 0 {
                return Ok(None);
            }
            let algorithm = Algorithm {
                bcrypt,
                handle: algorithm,
            };

            // 哈希对象的大小由具体实现决定，要先问再分配（不能写死）。
            let property_name = wide("ObjectLength");
            let mut object_length: u32 = 0;
            let mut returned: u32 = 0;
            if (bcrypt.get_property)(
                algorithm.handle,
                property_name.as_ptr(),
                (&mut object_length as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32,
                &mut returned,
                0,
            ) < 0
                || object_length == 0
                || object_length > 16 * 1024 * 1024
            {
                return Ok(None);
            }
            let mut object = vec![0_u8; object_length as usize];
            let mut hash: Handle = null_mut();
            if (bcrypt.create_hash)(
                algorithm.handle,
                &mut hash,
                object.as_mut_ptr(),
                object_length,
                null(),
                0,
                0,
            ) < 0
            {
                return Ok(None);
            }
            let hash = Hash {
                bcrypt,
                handle: hash,
            };

            // 接口中途报错不往上抛：记下来，读完之后按「不可用」处理，调用方回落到软件实现。
            let mut provider_failed = false;
            read_file_chunks(path, |chunk| {
                // 接口的单次长度参数是 32 位；读块远小于 4 GiB，这里只是防御。
                for part in chunk.chunks(u32::MAX as usize) {
                    if (bcrypt.hash_data)(hash.handle, part.as_ptr(), part.len() as u32, 0) < 0 {
                        provider_failed = true;
                        return Ok(());
                    }
                }
                Ok(())
            })?;
            if provider_failed {
                return Ok(None);
            }

            let mut digest = [0_u8; 32];
            if (bcrypt.finish_hash)(hash.handle, digest.as_mut_ptr(), digest.len() as u32, 0) < 0 {
                return Ok(None);
            }
            Ok(Some(hex(&digest)))
        }
    }

    fn hex(bytes: &[u8]) -> String {
        let mut text = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write as _;
            let _ = write!(text, "{byte:02x}");
        }
        text
    }

    #[cfg(test)]
    mod tests {
        use crate::task_store::sha256_file_software;
        use crate::{sha256_backend, sha256_file};
        use std::fs;

        fn temp_file(tag: &str, size: usize) -> std::path::PathBuf {
            use std::io::Write as _;
            let path = std::env::temp_dir().join(format!("br-cng-{tag}-{}", uuid::Uuid::new_v4()));
            let mut file = fs::File::create(&path).expect("create temp file");
            // 确定性的伪随机内容：同一个位置永远是同一个字节，与大小无关。
            let mut state = 0x2545_F491_4F6C_DD1D_u64;
            let mut remaining = size;
            let mut buffer = vec![0_u8; 1 << 20];
            while remaining > 0 {
                for slot in buffer.iter_mut() {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    *slot = state as u8;
                }
                let count = remaining.min(buffer.len());
                file.write_all(&buffer[..count]).expect("write temp file");
                remaining -= count;
            }
            path
        }

        #[test]
        fn bcrypt_and_software_agree_for_various_sizes() {
            // 0 与 1 字节、分块边界两侧、跨越 4 MiB 读块的边界，都要逐位一致。
            for (tag, size) in [
                ("empty", 0),
                ("one", 1),
                ("sixty-three", 63),
                ("sixty-four", 64),
                ("sixty-five", 65),
                ("four-kib", 4096),
                ("one-mib", 1 << 20),
                ("one-mib-plus-one", (1 << 20) + 1),
                ("three-mib-plus-seven", 3 * (1 << 20) + 7),
                ("five-mib", 5 * (1 << 20)),
            ] {
                let path = temp_file(tag, size);
                let fast = sha256_file(&path).expect("sha256_file");
                let slow = sha256_file_software(&path).expect("sha256_file_software");
                let bytes = fs::read(&path).expect("read temp file");
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(&bytes);
                let in_memory = format!("{:x}", hasher.finalize());
                fs::remove_file(&path).expect("remove temp file");
                assert_eq!(
                    fast, slow,
                    "size {size}: fast path differs from software path"
                );
                assert_eq!(
                    fast, in_memory,
                    "size {size}: both file paths differ from the in-memory reference"
                );
                assert_eq!(
                    fast.len(),
                    64,
                    "size {size}: digest must be 64 hex characters"
                );
            }
        }

        #[test]
        fn fast_path_is_actually_taken_on_windows() {
            // 这条用例守的是「悄悄一直走软件实现」：如果 bcrypt.dll 加载失败，
            // 上一条用例照样全绿（两边都是软件实现），速度优化却已经失效。
            assert_eq!(sha256_backend(), "bcrypt");
            let path = temp_file("backend", 1000);
            assert!(super::sha256_file(&path).expect("cng path").is_some());
            fs::remove_file(&path).expect("remove temp file");
        }
    }
}

#[cfg(not(windows))]
mod windows {
    pub(super) fn bcrypt_available() -> bool {
        false
    }
}
