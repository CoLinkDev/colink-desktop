#[derive(Clone)]
pub struct ProgressInfo {
    pub position_ms: Option<i64>,
    pub duration_ms: Option<i64>,
}

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::time::{Duration, Instant};

    use super::ProgressInfo;
    use windows::{
        core::{Interface, BSTR},
        Win32::{
            Foundation::{HWND, LPARAM, RPC_E_CHANGED_MODE},
            System::Com::{
                CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
                COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED,
            },
            UI::Accessibility::{
                CUIAutomation, IUIAutomation, IUIAutomationElement,
                IUIAutomationRangeValuePattern, TreeScope_Descendants, UIA_RangeValuePatternId,
                UIA_SliderControlTypeId,
            },
            UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, IsWindowVisible},
        },
    };

    const PROGRESS_NAME: &str = "进度";
    const KUGOU_TITLE_HINT: &str = "酷狗";
    const SLIDER_UNIT_MS: f64 = 10.0;
    const REFIND_INTERVAL: Duration = Duration::from_secs(10);

    thread_local! {
        static UIA_CACHE: RefCell<UiaThreadCache> = RefCell::new(UiaThreadCache::default());
    }

    pub struct ProgressReader {
        hwnd: Option<isize>,
        last_refind: Option<Instant>,
    }

    impl ProgressReader {
        pub fn new() -> Self {
            Self {
                hwnd: None,
                last_refind: None,
            }
        }

        pub fn clear(&mut self) {
            self.hwnd = None;
            self.last_refind = None;
        }

        pub fn read_progress(&mut self) -> Option<ProgressInfo> {
            if let Some(progress) = self.read_cached_window() {
                return Some(progress);
            }

            if !self.should_refind() {
                return None;
            }

            self.hwnd = find_kugou_window();
            self.last_refind = Some(Instant::now());
            self.read_cached_window()
        }

        fn should_refind(&self) -> bool {
            self.last_refind
                .map(|instant| instant.elapsed() >= REFIND_INTERVAL)
                .unwrap_or(true)
        }

        fn read_cached_window(&mut self) -> Option<ProgressInfo> {
            let hwnd = self.hwnd?;
            let progress = read_progress_from_window(hwnd);
            if progress.is_none() {
                self.hwnd = None;
            }
            progress
        }
    }

    fn read_progress_from_window(hwnd: isize) -> Option<ProgressInfo> {
        UIA_CACHE.with(|cache| cache.borrow_mut().read_progress(hwnd))
    }

    #[derive(Default)]
    struct UiaThreadCache {
        _com: Option<ComInit>,
        automation: Option<IUIAutomation>,
        slider: Option<CachedSlider>,
    }

    struct CachedSlider {
        hwnd: isize,
        element: IUIAutomationElement,
    }

    impl UiaThreadCache {
        fn read_progress(&mut self, hwnd: isize) -> Option<ProgressInfo> {
            if let Some(cached) = self.slider.as_ref().filter(|item| item.hwnd == hwnd) {
                if let Some(progress) = read_slider(&cached.element) {
                    return Some(progress);
                }
            }

            self.slider = None;
            let automation = self.automation()?;
            let window = unsafe { automation.ElementFromHandle(HWND(hwnd as *mut c_void)).ok()? };
            let slider = find_slider(&automation, &window)?;
            let progress = read_slider(&slider)?;
            self.slider = Some(CachedSlider {
                hwnd,
                element: slider,
            });
            Some(progress)
        }

        fn automation(&mut self) -> Option<IUIAutomation> {
            if self._com.is_none() {
                self._com = Some(init_com()?);
            }
            if self.automation.is_none() {
                self.automation = create_automation();
            }
            self.automation.clone()
        }
    }

    fn find_slider(
        automation: &IUIAutomation,
        window: &IUIAutomationElement,
    ) -> Option<IUIAutomationElement> {
        let condition = unsafe { automation.CreateTrueCondition().ok()? };
        let elements = unsafe { window.FindAll(TreeScope_Descendants, &condition).ok()? };
        let length = unsafe { elements.Length().ok()? };

        for index in 0..length {
            let element = unsafe { elements.GetElement(index).ok()? };
            if is_kugou_progress_slider(&element) {
                return Some(element);
            }
        }
        None
    }

    fn create_automation() -> Option<IUIAutomation> {
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok() }
    }

    struct ComInit;

    impl Drop for ComInit {
        fn drop(&mut self) {
            unsafe {
                CoUninitialize();
            }
        }
    }

    fn init_com() -> Option<ComInit> {
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if result == RPC_E_CHANGED_MODE {
            unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok()? };
            return Some(ComInit);
        }
        result.ok().ok()?;
        Some(ComInit)
    }

    fn find_kugou_window() -> Option<isize> {
        let mut matches = Vec::<isize>::new();
        unsafe {
            let _ = EnumWindows(
                Some(enum_window),
                LPARAM((&mut matches as *mut Vec<isize>) as isize),
            );
        }
        matches.into_iter().next()
    }

    unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        if unsafe { IsWindowVisible(hwnd).as_bool() } && window_title(hwnd).contains(KUGOU_TITLE_HINT)
        {
            let matches = unsafe { &mut *(lparam.0 as *mut Vec<isize>) };
            matches.push(hwnd.0 as isize);
        }
        true.into()
    }

    fn window_title(hwnd: HWND) -> String {
        let mut buffer = vec![0_u16; 512];
        let len = unsafe { GetWindowTextW(hwnd, &mut buffer) };
        if len <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buffer[..len as usize])
            .trim()
            .to_string()
    }

    fn is_kugou_progress_slider(element: &IUIAutomationElement) -> bool {
        let Ok(control_type) = (unsafe { element.CurrentControlType() }) else {
            return false;
        };
        if control_type != UIA_SliderControlTypeId {
            return false;
        }

        let name = unsafe { element.CurrentName().ok() };
        bstr_text(name.as_ref()).as_deref() == Some(PROGRESS_NAME)
    }

    fn read_slider(element: &IUIAutomationElement) -> Option<ProgressInfo> {
        if !is_kugou_progress_slider(element) {
            return None;
        }

        let pattern = unsafe { element.GetCurrentPattern(UIA_RangeValuePatternId).ok()? };
        let pattern = pattern.cast::<IUIAutomationRangeValuePattern>().ok()?;
        let value = unsafe { pattern.CurrentValue().ok()? };
        let maximum = unsafe { pattern.CurrentMaximum().ok()? };

        Some(ProgressInfo {
            position_ms: finite_positive(value).map(|value| (value * SLIDER_UNIT_MS).round() as i64),
            duration_ms: finite_positive(maximum)
                .map(|value| (value * SLIDER_UNIT_MS).round() as i64),
        })
    }

    fn finite_positive(value: f64) -> Option<f64> {
        value.is_finite().then_some(value).filter(|value| *value > 0.0)
    }

    fn bstr_text(value: Option<&BSTR>) -> Option<String> {
        value
            .map(ToString::to_string)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }
}

#[cfg(not(windows))]
mod imp {
    use super::ProgressInfo;

    pub struct ProgressReader;

    impl ProgressReader {
        pub fn new() -> Self {
            Self
        }

        pub fn clear(&mut self) {}

        pub fn read_progress(&mut self) -> Option<ProgressInfo> {
            None
        }
    }
}

pub use imp::ProgressReader;
