//! Windows virtual-file clipboard support.
//!
//! Explorer receives file descriptors immediately and asks each `IStream` for
//! bytes only when the destination copy actually starts. Reads come from the
//! verified content cache, or from the growing partial download while the
//! transfer is still streaming in.

use crate::{
    content::{is_file_id, ClipboardEntry, FileInfo, TreeNode},
    download_path, snapshot_entry, AppState,
};
use serde::Serialize;
use std::{
    ffi::c_void,
    fs,
    io::{Read, Seek, SeekFrom},
    mem::{size_of, ManuallyDrop},
    ptr,
    sync::{mpsc, Mutex, OnceLock},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager};
use windows::{
    core::{implement, Error, Ref, Result as WinResult, HRESULT},
    Win32::{
        Foundation::{
            DV_E_FORMATETC, DV_E_LINDEX, E_NOTIMPL, OLE_E_ADVISENOTSUPPORTED, STG_E_ACCESSDENIED,
            STG_E_INVALIDFUNCTION, STG_E_READFAULT, STG_E_SEEKERROR,
        },
        System::{
            Com::{
                IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA,
                ISequentialStream_Impl, IStream, IStream_Impl, DATADIR_GET, DVASPECT_CONTENT,
                FORMATETC, LOCKTYPE, STATFLAG, STATSTG, STGC, STGM, STGMEDIUM, STGMEDIUM_0,
                STGTY_STREAM, STREAM_SEEK, STREAM_SEEK_CUR, STREAM_SEEK_END, STREAM_SEEK_SET,
                TYMED_HGLOBAL, TYMED_ISTREAM,
            },
            DataExchange::RegisterClipboardFormatW,
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GHND},
            Ole::{OleInitialize, OleSetClipboard, OleUninitialize},
        },
        UI::Shell::{
            SHCreateStdEnumFmtEtc, CFSTR_FILECONTENTS, CFSTR_FILEDESCRIPTORW, FD_ATTRIBUTES,
            FD_FILESIZE, FD_PROGRESSUI, FD_UNICODE, FILEDESCRIPTORW,
        },
        UI::WindowsAndMessaging::{
            DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
        },
    },
};

const FILE_ATTRIBUTE_DIRECTORY_VALUE: u32 = 0x10;
const FILE_ATTRIBUTE_NORMAL_VALUE: u32 = 0x80;
const STREAM_WAIT_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone)]
struct VirtualItem {
    relative_path: String,
    file_id: Option<String>,
    size: Option<u64>,
    is_dir: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct VirtualFileRequest {
    entry_id: String,
    file_id: String,
    size: u64,
    source_device_id: String,
}

fn format_etc(format: u16, lindex: i32, tymed: i32) -> FORMATETC {
    FORMATETC {
        cfFormat: format,
        ptd: ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex,
        tymed: tymed as u32,
    }
}

fn clipboard_formats() -> (u16, u16) {
    unsafe {
        (
            RegisterClipboardFormatW(CFSTR_FILEDESCRIPTORW) as u16,
            RegisterClipboardFormatW(CFSTR_FILECONTENTS) as u16,
        )
    }
}

fn normalize_descriptor_path(path: &str) -> Result<String, String> {
    if path.is_empty()
        || path.starts_with('/')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path.contains('\0')
    {
        return Err(format!("虚拟文件路径不合法：{path}"));
    }
    let path = path.replace('/', "\\");
    if path.encode_utf16().count() >= 260 {
        return Err(format!("虚拟文件路径超过 Windows 限制：{path}"));
    }
    Ok(path)
}

fn file_item(path: &str, f: &str, size: u64) -> Result<VirtualItem, String> {
    if !is_file_id(f) {
        return Err(format!("文件内容尚未准备好：{path}"));
    }
    Ok(VirtualItem {
        relative_path: normalize_descriptor_path(path)?,
        file_id: Some(f.to_string()),
        size: Some(size),
        is_dir: false,
    })
}

fn virtual_items(entry: &ClipboardEntry) -> Result<Vec<VirtualItem>, String> {
    let file_info = entry
        .file_info
        .as_ref()
        .ok_or_else(|| "该记录不包含文件".to_string())?;
    let mut items = Vec::new();
    fn walk(dir: &FileInfo, prefix: &str, items: &mut Vec<VirtualItem>) -> Result<(), String> {
        for (name, node) in dir {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            match node {
                TreeNode::File { f, s } => items.push(file_item(&path, f, *s)?),
                TreeNode::Dir(children) => {
                    items.push(VirtualItem {
                        relative_path: normalize_descriptor_path(&path)?,
                        file_id: None,
                        size: Some(0),
                        is_dir: true,
                    });
                    walk(children, &path, items)?;
                }
            }
        }
        Ok(())
    }
    walk(file_info, "", &mut items)?;
    if items.is_empty() {
        return Err("该记录不包含文件".to_string());
    }
    Ok(items)
}

/// Entries that cannot be represented by FILEDESCRIPTORW (for example paths
/// beyond its fixed MAX_PATH field) fall back to the shared materialized-path
/// strategy instead of failing after the user starts a paste.
pub fn supports_entry(entry: &ClipboardEntry) -> bool {
    virtual_items(entry).is_ok()
}

fn descriptor(item: &VirtualItem) -> FILEDESCRIPTORW {
    let mut value = FILEDESCRIPTORW::default();
    let mut flags = FD_ATTRIBUTES.0 as u32 | FD_PROGRESSUI.0 as u32 | FD_UNICODE.0 as u32;
    if !item.is_dir && item.size.is_some() {
        flags |= FD_FILESIZE.0 as u32;
    }
    value.dwFlags = flags;
    value.dwFileAttributes = if item.is_dir {
        FILE_ATTRIBUTE_DIRECTORY_VALUE
    } else {
        FILE_ATTRIBUTE_NORMAL_VALUE
    };
    let size = item.size.unwrap_or_default();
    value.nFileSizeHigh = (size >> 32) as u32;
    value.nFileSizeLow = size as u32;
    let mut file_name = [0u16; 260];
    for (target, source) in file_name.iter_mut().zip(item.relative_path.encode_utf16()) {
        *target = source;
    }
    value.cFileName = file_name;
    value
}

fn descriptor_medium(items: &[VirtualItem]) -> WinResult<STGMEDIUM> {
    let bytes = size_of::<u32>() + items.len() * size_of::<FILEDESCRIPTORW>();
    unsafe {
        let global = GlobalAlloc(GHND, bytes)?;
        let memory = GlobalLock(global);
        if memory.is_null() {
            return Err(Error::from_hresult(STG_E_READFAULT));
        }
        ptr::write_unaligned(memory.cast::<u32>(), items.len() as u32);
        let mut cursor = memory.cast::<u8>().add(size_of::<u32>());
        for item in items {
            ptr::write_unaligned(cursor.cast::<FILEDESCRIPTORW>(), descriptor(item));
            cursor = cursor.add(size_of::<FILEDESCRIPTORW>());
        }
        let _ = GlobalUnlock(global);
        Ok(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: global },
            pUnkForRelease: ManuallyDrop::new(None),
        })
    }
}

#[implement(IStream)]
struct VirtualFileStream {
    app: AppHandle,
    window_label: String,
    entry_id: String,
    source_device_id: String,
    file_id: String,
    size: Option<u64>,
    position: Mutex<u64>,
}

impl VirtualFileStream {
    fn create(
        app: AppHandle,
        window_label: String,
        entry_id: String,
        source_device_id: String,
        item: &VirtualItem,
        position: u64,
    ) -> IStream {
        Self {
            app,
            window_label,
            entry_id,
            source_device_id,
            file_id: item.file_id.clone().unwrap_or_default(),
            size: item.size,
            position: Mutex::new(position),
        }
        .into()
    }

    fn request_download(&self) -> Result<(), String> {
        let state = self.app.state::<AppState>();
        let should_emit = state.virtual_downloads.request(&self.file_id);
        if should_emit {
            let transfer_size = self.size.unwrap_or_default();
            self.app
                .emit_to(
                    &self.window_label,
                    "cliproam://virtual-file-request",
                    VirtualFileRequest {
                        entry_id: self.entry_id.clone(),
                        file_id: self.file_id.clone(),
                        size: transfer_size,
                        source_device_id: self.source_device_id.clone(),
                    },
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn resolved_path(&self) -> Option<std::path::PathBuf> {
        let state = self.app.state::<AppState>();
        let snapshot = snapshot_entry(&state, &self.entry_id).ok()?;
        snapshot.resolve(&self.file_id)
    }

    fn read_at(&self, target: &mut [u8], position: u64) -> Result<usize, String> {
        if let Some(path) = self.resolved_path() {
            let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
            file.seek(SeekFrom::Start(position))
                .map_err(|error| error.to_string())?;
            return file.read(target).map_err(|error| error.to_string());
        }

        self.request_download()?;
        let state = self.app.state::<AppState>();
        loop {
            if let Some(path) = self.resolved_path() {
                let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
                file.seek(SeekFrom::Start(position))
                    .map_err(|error| error.to_string())?;
                return file.read(target).map_err(|error| error.to_string());
            }

            let snapshot = snapshot_entry(&state, &self.entry_id)?;
            let partial = download_path(&snapshot.cache_dir, &self.file_id)
                .ok_or_else(|| "内容标识不合法".to_string())?;
            if let Ok(length) = fs::metadata(&partial).map(|metadata| metadata.len()) {
                if length > position {
                    let mut file =
                        fs::File::open(&partial).map_err(|error| error.to_string())?;
                    file.seek(SeekFrom::Start(position))
                        .map_err(|error| error.to_string())?;
                    let available =
                        usize::try_from((length - position).min(target.len() as u64))
                            .unwrap_or(target.len());
                    return file
                        .read(&mut target[..available])
                        .map_err(|error| error.to_string());
                }
            }

            let mut transfers = state
                .virtual_downloads
                .transfers
                .lock()
                .map_err(|e| e.to_string())?;
            if let Some(status) = transfers.get(&self.file_id) {
                if let Some(error) = &status.error {
                    return Err(error.clone());
                }
                if status.complete {
                    return Ok(0);
                }
            }
            let (guard, timeout) = state
                .virtual_downloads
                .changed
                .wait_timeout(transfers, STREAM_WAIT_TIMEOUT)
                .map_err(|error| error.to_string())?;
            transfers = guard;
            if timeout.timed_out() {
                return Err("等待远端文件数据超时".to_string());
            }
            drop(transfers);
        }
    }
}

impl ISequentialStream_Impl for VirtualFileStream_Impl {
    fn Read(&self, pv: *mut c_void, cb: u32, pcbread: *mut u32) -> HRESULT {
        if !pcbread.is_null() {
            unsafe { *pcbread = 0 };
        }
        if cb == 0 {
            return HRESULT(0);
        }
        if pv.is_null() {
            return STG_E_READFAULT;
        }
        let mut position = match self.position.lock() {
            Ok(position) => position,
            Err(_) => return STG_E_READFAULT,
        };
        let target = unsafe { std::slice::from_raw_parts_mut(pv.cast::<u8>(), cb as usize) };
        match self.read_at(target, *position) {
            Ok(count) => {
                *position += count as u64;
                if !pcbread.is_null() {
                    unsafe { *pcbread = count as u32 };
                }
                if count == cb as usize {
                    HRESULT(0)
                } else {
                    HRESULT(1)
                }
            }
            Err(_) => STG_E_READFAULT,
        }
    }

    fn Write(&self, _pv: *const c_void, _cb: u32, pcbwritten: *mut u32) -> HRESULT {
        if !pcbwritten.is_null() {
            unsafe { *pcbwritten = 0 };
        }
        STG_E_ACCESSDENIED
    }
}

impl IStream_Impl for VirtualFileStream_Impl {
    fn Seek(&self, move_by: i64, origin: STREAM_SEEK, new_position: *mut u64) -> WinResult<()> {
        let mut position = self
            .position
            .lock()
            .map_err(|_| Error::from_hresult(STG_E_SEEKERROR))?;
        let base = if origin == STREAM_SEEK_SET {
            0i128
        } else if origin == STREAM_SEEK_CUR {
            *position as i128
        } else if origin == STREAM_SEEK_END {
            self.size
                .ok_or_else(|| Error::from_hresult(STG_E_SEEKERROR))? as i128
        } else {
            return Err(Error::from_hresult(STG_E_INVALIDFUNCTION));
        };
        let next = base + move_by as i128;
        if next < 0 || next > u64::MAX as i128 {
            return Err(Error::from_hresult(STG_E_SEEKERROR));
        }
        *position = next as u64;
        if !new_position.is_null() {
            unsafe { *new_position = *position };
        }
        Ok(())
    }

    fn SetSize(&self, _new_size: u64) -> WinResult<()> {
        Err(Error::from_hresult(STG_E_ACCESSDENIED))
    }

    fn CopyTo(
        &self,
        _stream: Ref<'_, IStream>,
        _cb: u64,
        _read: *mut u64,
        _written: *mut u64,
    ) -> WinResult<()> {
        Err(Error::from_hresult(E_NOTIMPL))
    }

    fn Commit(&self, _flags: &STGC) -> WinResult<()> {
        Ok(())
    }
    fn Revert(&self) -> WinResult<()> {
        Err(Error::from_hresult(E_NOTIMPL))
    }
    fn LockRegion(&self, _offset: u64, _cb: u64, _lock_type: &LOCKTYPE) -> WinResult<()> {
        Err(Error::from_hresult(STG_E_INVALIDFUNCTION))
    }
    fn UnlockRegion(&self, _offset: u64, _cb: u64, _lock_type: u32) -> WinResult<()> {
        Err(Error::from_hresult(STG_E_INVALIDFUNCTION))
    }

    fn Stat(&self, stat: *mut STATSTG, _flags: &STATFLAG) -> WinResult<()> {
        if stat.is_null() {
            return Err(Error::from_hresult(STG_E_READFAULT));
        }
        unsafe {
            ptr::write(
                stat,
                STATSTG {
                    r#type: STGTY_STREAM.0 as u32,
                    cbSize: self.size.unwrap_or_default(),
                    grfMode: STGM(0),
                    ..Default::default()
                },
            );
        }
        Ok(())
    }

    fn Clone(&self) -> WinResult<IStream> {
        let position = *self
            .position
            .lock()
            .map_err(|_| Error::from_hresult(STG_E_READFAULT))?;
        Ok(VirtualFileStream {
            app: self.app.clone(),
            window_label: self.window_label.clone(),
            entry_id: self.entry_id.clone(),
            source_device_id: self.source_device_id.clone(),
            file_id: self.file_id.clone(),
            size: self.size,
            position: Mutex::new(position),
        }
        .into())
    }
}

#[implement(IDataObject)]
struct VirtualFileDataObject {
    app: AppHandle,
    window_label: String,
    entry_id: String,
    source_device_id: String,
    items: Vec<VirtualItem>,
    descriptor_format: u16,
    contents_format: u16,
}

impl IDataObject_Impl for VirtualFileDataObject_Impl {
    fn GetData(&self, requested: *const FORMATETC) -> WinResult<STGMEDIUM> {
        if requested.is_null() {
            return Err(Error::from_hresult(DV_E_FORMATETC));
        }
        let requested = unsafe { &*requested };
        if requested.cfFormat == self.descriptor_format
            && requested.tymed & TYMED_HGLOBAL.0 as u32 != 0
        {
            return descriptor_medium(&self.items);
        }
        if requested.cfFormat == self.contents_format
            && requested.tymed & TYMED_ISTREAM.0 as u32 != 0
        {
            let index = usize::try_from(requested.lindex)
                .ok()
                .filter(|index| *index < self.items.len())
                .ok_or_else(|| Error::from_hresult(DV_E_LINDEX))?;
            let item = &self.items[index];
            if item.is_dir {
                return Err(Error::from_hresult(DV_E_LINDEX));
            }
            let stream = VirtualFileStream::create(
                self.app.clone(),
                self.window_label.clone(),
                self.entry_id.clone(),
                self.source_device_id.clone(),
                item,
                0,
            );
            return Ok(STGMEDIUM {
                tymed: TYMED_ISTREAM.0 as u32,
                u: STGMEDIUM_0 {
                    pstm: ManuallyDrop::new(Some(stream)),
                },
                pUnkForRelease: ManuallyDrop::new(None),
            });
        }
        Err(Error::from_hresult(DV_E_FORMATETC))
    }

    fn GetDataHere(&self, _format: *const FORMATETC, _medium: *mut STGMEDIUM) -> WinResult<()> {
        Err(Error::from_hresult(E_NOTIMPL))
    }

    fn QueryGetData(&self, requested: *const FORMATETC) -> HRESULT {
        if requested.is_null() {
            return DV_E_FORMATETC;
        }
        let requested = unsafe { &*requested };
        if requested.cfFormat == self.descriptor_format
            && requested.tymed & TYMED_HGLOBAL.0 as u32 != 0
        {
            return HRESULT(0);
        }
        if requested.cfFormat == self.contents_format
            && requested.tymed & TYMED_ISTREAM.0 as u32 != 0
            && requested.lindex >= 0
            && self
                .items
                .get(requested.lindex as usize)
                .is_some_and(|item| !item.is_dir)
        {
            return HRESULT(0);
        }
        DV_E_FORMATETC
    }

    fn GetCanonicalFormatEtc(&self, _input: *const FORMATETC, output: *mut FORMATETC) -> HRESULT {
        if !output.is_null() {
            unsafe { (*output).ptd = ptr::null_mut() };
        }
        E_NOTIMPL
    }

    fn SetData(
        &self,
        _format: *const FORMATETC,
        _medium: *const STGMEDIUM,
        _release: windows::core::BOOL,
    ) -> WinResult<()> {
        Err(Error::from_hresult(E_NOTIMPL))
    }

    fn EnumFormatEtc(&self, direction: u32) -> WinResult<IEnumFORMATETC> {
        if direction != DATADIR_GET.0 as u32 {
            return Err(Error::from_hresult(E_NOTIMPL));
        }
        unsafe {
            SHCreateStdEnumFmtEtc(&[
                format_etc(self.descriptor_format, -1, TYMED_HGLOBAL.0),
                format_etc(self.contents_format, -1, TYMED_ISTREAM.0),
            ])
        }
    }

    fn DAdvise(
        &self,
        _format: *const FORMATETC,
        _flags: u32,
        _sink: Ref<'_, IAdviseSink>,
    ) -> WinResult<u32> {
        Err(Error::from_hresult(OLE_E_ADVISENOTSUPPORTED))
    }
    fn DUnadvise(&self, _connection: u32) -> WinResult<()> {
        Err(Error::from_hresult(OLE_E_ADVISENOTSUPPORTED))
    }
    fn EnumDAdvise(&self) -> WinResult<IEnumSTATDATA> {
        Err(Error::from_hresult(OLE_E_ADVISENOTSUPPORTED))
    }
}

static CLIPBOARD_THREAD: OnceLock<Mutex<Option<mpsc::SyncSender<()>>>> = OnceLock::new();

pub fn initialize() -> Result<(), String> {
    Ok(())
}

pub fn set_clipboard(
    app: &AppHandle,
    window_label: &str,
    entry: ClipboardEntry,
) -> Result<(), String> {
    let items = virtual_items(&entry)?;
    let (descriptor_format, contents_format) = clipboard_formats();
    if descriptor_format == 0 || contents_format == 0 {
        return Err("无法注册 Windows 虚拟文件剪贴板格式".to_string());
    }
    let data = VirtualFileDataObject {
        app: app.clone(),
        window_label: window_label.to_string(),
        entry_id: entry.id,
        source_device_id: entry.source_device_id,
        items,
        descriptor_format,
        contents_format,
    };
    let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
    let (stop_sender, stop_receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("cliproam-virtual-clipboard".to_string())
        .spawn(move || {
            let initialized = unsafe { OleInitialize(None) }.map_err(|error| error.to_string());
            if let Err(error) = initialized {
                let _ = ready_sender.send(Err(error));
                return;
            }
            let object: IDataObject = data.into();
            let result = unsafe { OleSetClipboard(&object) }.map_err(|error| error.to_string());
            let running = result.is_ok();
            let _ = ready_sender.send(result);
            while running && stop_receiver.try_recv().is_err() {
                unsafe {
                    let mut message = MSG::default();
                    while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                std::thread::sleep(Duration::from_millis(8));
            }
            unsafe { OleUninitialize() };
        })
        .map_err(|error| error.to_string())?;
    ready_receiver
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "设置虚拟文件剪贴板超时".to_string())??;
    let slot = CLIPBOARD_THREAD.get_or_init(|| Mutex::new(None));
    let mut current = slot.lock().map_err(|error| error.to_string())?;
    if let Some(previous) = current.replace(stop_sender) {
        let _ = previous.send(());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::normalize_descriptor_path;

    #[test]
    fn descriptor_paths_fall_back_before_the_fixed_filename_buffer_overflows() {
        assert!(normalize_descriptor_path(&"a".repeat(259)).is_ok());
        assert!(normalize_descriptor_path(&"a".repeat(260)).is_err());
        assert!(normalize_descriptor_path("root/../outside").is_err());
    }
}
