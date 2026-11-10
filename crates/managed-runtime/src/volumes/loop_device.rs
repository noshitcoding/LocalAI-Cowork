//! Linux UAPI, see include/uapi/linux/loop.h (Linux-syscall-note). Only loops
//! bound to our exact private image inode and nonce can be changed or detached.
use super::*;
use std::os::unix::fs::FileTypeExt;

const LOOP_CONFIGURE: libc::c_ulong = 0x4c0a;
const LOOP_GET_STATUS64: libc::c_ulong = 0x4c05;
const LOOP_CLR_FD: libc::c_ulong = 0x4c01;
const LOOP_CTL_GET_FREE: libc::c_ulong = 0x4c82;
const AUTOCLEAR: u32 = 4;

#[repr(C)]
struct LoopInfo {
    device: u64,
    inode: u64,
    rdevice: u64,
    offset: u64,
    sizelimit: u64,
    number: u32,
    encrypt_type: u32,
    encrypt_key_size: u32,
    flags: u32,
    filename: [u8; 64],
    crypt_name: [u8; 64],
    encrypt_key: [u8; 32],
    init: [u64; 2],
}
impl Default for LoopInfo {
    fn default() -> Self {
        // All fields are integer values or integer arrays in the Linux UAPI.
        unsafe { std::mem::zeroed() }
    }
}
#[derive(Default)]
#[repr(C)]
struct LoopConfig {
    fd: u32,
    block_size: u32,
    info: LoopInfo,
    reserved: [u64; 8],
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct LoopIdentity {
    pub number: u32,
    pub rdev: u64,
}
pub(super) struct LoopDevice {
    file: File,
    pub identity: LoopIdentity,
}
impl LoopDevice {
    pub fn attach(image: &File, nonce: &str, bytes: u64) -> Result<Self> {
        let control = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open("/dev/loop-control")?;
        let image_id = NativeId::file(image)?;
        for _ in 0..16 {
            // SAFETY: this ioctl takes no pointer and returns a device number.
            let number = unsafe { libc::ioctl(control.as_raw_fd(), LOOP_CTL_GET_FREE) };
            if number < 0 {
                return Err(io::Error::last_os_error().into());
            }
            let mut device = Self::open(number as u32, true)?;
            let mut info = LoopInfo {
                sizelimit: bytes,
                flags: AUTOCLEAR,
                ..Default::default()
            };
            if nonce.len() > info.filename.len() {
                return Err(VolumeError::Identity);
            }
            info.filename[..nonce.len()].copy_from_slice(nonce.as_bytes());
            let config = LoopConfig {
                fd: image.as_raw_fd() as u32,
                block_size: 4096,
                info,
                reserved: [0; 8],
            };
            // SAFETY: kernel consumes the live image fd and the exact UAPI layout.
            if unsafe { libc::ioctl(device.file.as_raw_fd(), LOOP_CONFIGURE, &config) } < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EBUSY) {
                    continue;
                }
                return Err(error.into());
            }
            device.identity.rdev = device.file.metadata()?.rdev();
            device.require(&image_id, nonce, bytes)?;
            return Ok(device);
        }
        Err(VolumeError::Busy)
    }
    pub fn reopen(
        identity: &LoopIdentity,
        image: &NativeId,
        nonce: &str,
        bytes: u64,
    ) -> Result<Self> {
        let device = Self::open(identity.number, true)?;
        if device.identity != *identity {
            return Err(VolumeError::Identity);
        }
        device.require(image, nonce, bytes)?;
        Ok(device)
    }
    fn open(number: u32, write: bool) -> Result<Self> {
        if number > 1_048_575 {
            return Err(VolumeError::Identity);
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(write)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(format!("/dev/loop{number}"))?;
        let m = file.metadata()?;
        if !m.file_type().is_block_device()
            || m.uid() != 0
            || libc::major(m.rdev()) != 7
            || libc::minor(m.rdev()) != number
        {
            return Err(VolumeError::Identity);
        }
        Ok(Self {
            identity: LoopIdentity {
                number,
                rdev: m.rdev(),
            },
            file,
        })
    }
    fn info(&self) -> Result<Option<LoopInfo>> {
        let mut info = LoopInfo::default();
        // SAFETY: writable, correctly aligned UAPI output remains live.
        if unsafe { libc::ioctl(self.file.as_raw_fd(), LOOP_GET_STATUS64, &mut info) } < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENXIO) {
                return Ok(None);
            }
            return Err(error.into());
        }
        Ok(Some(info))
    }
    pub(super) fn require(&self, image: &NativeId, nonce: &str, bytes: u64) -> Result<()> {
        let info = self.info()?.ok_or(VolumeError::Identity)?;
        let mut expected = [0u8; 64];
        if nonce.len() > expected.len() {
            return Err(VolumeError::Identity);
        }
        expected[..nonce.len()].copy_from_slice(nonce.as_bytes());
        if info.device != image.dev
            || info.inode != image.ino
            || info.offset != 0
            || info.sizelimit != bytes
            || info.number != self.identity.number
            || info.filename != expected
            || info.flags != AUTOCLEAR
            || info.encrypt_type != 0
            || info.encrypt_key_size != 0
        {
            return Err(VolumeError::Identity);
        }
        Ok(())
    }
    pub fn source(&self) -> PathBuf {
        fd_path(self.file.as_raw_fd())
    }
    pub fn detach(self, image: &NativeId, nonce: &str, bytes: u64) -> Result<()> {
        self.require(image, nonce, bytes)?;
        // A successful CLR_FD alone is insufficient: with live references the
        // kernel may merely set AUTOCLEAR and keep the physical device attached.
        if unsafe { libc::ioctl(self.file.as_raw_fd(), LOOP_CLR_FD) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        if self.info()?.is_some() {
            return Err(VolumeError::StillAttached);
        }
        Ok(())
    }
}

pub(super) fn owned(image: &NativeId, nonce: &str, bytes: u64) -> Result<Vec<LoopDevice>> {
    let mut result = Vec::new();
    scan(image, |device, info| {
        device.require(image, nonce, bytes)?;
        let identity = LoopIdentity {
            number: info.number,
            rdev: device.file.metadata()?.rdev(),
        };
        result.push(LoopDevice::reopen(&identity, image, nonce, bytes)?);
        Ok(())
    })?;
    Ok(result)
}
pub(super) fn associated(image: &NativeId) -> Result<bool> {
    let mut found = false;
    scan(image, |_, _| {
        found = true;
        Ok(())
    })?;
    Ok(found)
}
fn scan(
    image: &NativeId,
    mut found: impl FnMut(&LoopDevice, &LoopInfo) -> Result<()>,
) -> Result<()> {
    let mut count = 0;
    for entry in std::fs::read_dir("/sys/block")? {
        let name = entry?.file_name();
        let Some(number) = name
            .to_str()
            .and_then(|s| s.strip_prefix("loop"))
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        count += 1;
        if count > 4096 {
            return Err(VolumeError::Uncertain);
        }
        let device = match LoopDevice::open(number, false) {
            Ok(device) => device,
            Err(VolumeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if let Some(info) = device.info()?
            && info.device == image.dev
            && info.inode == image.ino
        {
            found(&device, &info)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linux_uapi_sizes_match_64_bit_kernel_headers() {
        assert_eq!(std::mem::size_of::<LoopInfo>(), 232);
        assert_eq!(std::mem::size_of::<LoopConfig>(), 304);
    }
}
