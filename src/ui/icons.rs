//! App icon cache: shell bitmaps converted to premultiplied 32-bit DIBs and
//! drawn with `AlphaBlend`.

use std::collections::HashMap;

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, DeleteObject,
    GetDC, GetDIBits, GetObjectW, HBITMAP, ReleaseDC,
};

/// Raw icon pixels (premultiplied BGRA, top-down) produced off the UI thread.
pub struct IconPixels {
    pub id: String,
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
}

fn bmi(w: i32, h: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Reads an HBITMAP into premultiplied BGRA pixels, fixing up bitmaps that
/// have no alpha channel or straight (non-premultiplied) alpha.
pub unsafe fn hbitmap_to_pixels(hbmp: HBITMAP) -> Option<IconPixels> {
    let mut bm = BITMAP::default();
    let got = unsafe {
        GetObjectW(hbmp.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _))
    };
    if got == 0 || bm.bmWidth <= 0 || bm.bmHeight == 0 {
        return None;
    }
    let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
    let mut info = bmi(w, h);
    let mut px = vec![0u32; (w * h) as usize];
    let lines = unsafe {
        let hdc = GetDC(None);
        let n = GetDIBits(hdc, hbmp, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
        ReleaseDC(None, hdc);
        n
    };
    if lines == 0 {
        return None;
    }

    let has_alpha = px.iter().any(|p| p >> 24 != 0);
    if !has_alpha {
        for p in px.iter_mut() {
            *p |= 0xFF00_0000;
        }
    } else {
        let premultiplied = px.iter().all(|&p| {
            let a = p >> 24;
            (p >> 16 & 0xFF) <= a && (p >> 8 & 0xFF) <= a && (p & 0xFF) <= a
        });
        if !premultiplied {
            for p in px.iter_mut() {
                let a = *p >> 24;
                let m = |c: u32| (c * a + 127) / 255;
                *p = a << 24 | m(*p >> 16 & 0xFF) << 16 | m(*p >> 8 & 0xFF) << 8 | m(*p & 0xFF);
            }
        }
    }
    Some(IconPixels { id: String::new(), w, h, px })
}

#[derive(Clone, Copy)]
pub struct CachedIcon {
    pub bmp: HBITMAP,
    pub w: i32,
    pub h: i32,
}

thread_local! {
    /// One cache for the UI thread, shared by the launcher and Settings.
    static SHARED: std::cell::RefCell<IconCache> = std::cell::RefCell::new(IconCache::default());
}

/// Runs `f` with the shared icon cache (UI thread only; don't nest).
pub fn with<R>(f: impl FnOnce(&mut IconCache) -> R) -> R {
    SHARED.with(|c| f(&mut c.borrow_mut()))
}

/// Copy of an app's icon handle, if loaded.
pub fn get(id: &str) -> Option<CachedIcon> {
    with(|c| c.get(id).copied())
}

#[derive(Default)]
pub struct IconCache {
    map: HashMap<String, CachedIcon>,
}

impl IconCache {
    pub fn insert(&mut self, icon: IconPixels) {
        let info = bmi(icon.w, icon.h);
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(bmp) = (unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }) else {
            return;
        };
        if bits.is_null() {
            return;
        }
        unsafe { std::ptr::copy_nonoverlapping(icon.px.as_ptr(), bits as *mut u32, icon.px.len()) };
        if let Some(old) = self.map.insert(icon.id, CachedIcon { bmp, w: icon.w, h: icon.h }) {
            unsafe {
                let _ = DeleteObject(old.bmp.into());
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&CachedIcon> {
        self.map.get(id)
    }

    pub fn ids(&self) -> std::collections::HashSet<String> {
        self.map.keys().cloned().collect()
    }

    pub fn clear(&mut self) {
        for (_, ic) in self.map.drain() {
            unsafe {
                let _ = DeleteObject(ic.bmp.into());
            }
        }
    }
}

impl Drop for IconCache {
    fn drop(&mut self) {
        self.clear();
    }
}
