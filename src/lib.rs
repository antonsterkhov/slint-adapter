#![no_std]
#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

extern crate alloc;

use alloc::{boxed::Box, format, rc::Rc, vec, vec::Vec};
use core::{cell::RefCell, fmt, time::Duration};

use embedded_graphics_core::{
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Point, Size},
    pixelcolor::{Rgb565, raw::RawU16},
    primitives::Rectangle,
};
use embedded_hal::{delay::DelayNs, digital::OutputPin};
use mipidsi::{
    interface::{Interface, InterfacePixelFormat},
    models::Model,
};
use slint::{
    LogicalPosition, PhysicalSize,
    platform::{
        self, Platform, PlatformError, PointerEventButton, WindowAdapter, WindowEvent,
        software_renderer::{
            LineBufferProvider, MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel,
        },
    },
};

/// A touch position in the logical pixel coordinate system of the configured display.
///
/// `(0, 0)` is the top-left corner. A touch driver is responsible for calibration and
/// for applying the same rotation/mirroring that was configured on the `mipidsi`
/// [`mipidsi::Builder`]. Coordinates outside the display are clamped by the adapter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TouchPoint {
    /// Horizontal position in logical pixels.
    pub x: u16,
    /// Vertical position in logical pixels.
    pub y: u16,
}

impl TouchPoint {
    /// Creates a touch point from logical display coordinates.
    #[must_use]
    pub const fn new(x: u16, y: u16) -> Self {
        Self { x, y }
    }
}

/// A polling touch-screen abstraction.
///
/// Return `Ok(Some(point))` while a finger or stylus is touching the screen and
/// `Ok(None)` when there is no contact. The adapter converts the sampled states into
/// Slint pointer press, move, release, and exit events.
///
/// Implementations should return coordinates in the logical coordinate system of the
/// already configured display. This keeps controller-specific calibration, axis swapping,
/// and inversion in the board-support layer where the raw touch range is known.
pub trait TouchInput {
    /// Error produced while sampling the touch controller.
    type Error;

    /// Samples the current primary contact.
    fn read_touch(&mut self) -> Result<Option<TouchPoint>, Self::Error>;
}

/// A touch provider for displays without a touch controller.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoTouch;

impl TouchInput for NoTouch {
    type Error = core::convert::Infallible;

    fn read_touch(&mut self) -> Result<Option<TouchPoint>, Self::Error> {
        Ok(None)
    }
}

/// Time and waiting operations required by the MCU event loop.
///
/// `now` must use a monotonic clock. `wait` may busy-wait, arm a hardware timer and
/// execute WFI, or delegate to an RTOS. It must return no later than needed for the
/// supplied duration; returning early is valid and is useful when a touch interrupt wakes
/// the MCU.
pub trait McuRuntime {
    /// Returns a monotonically increasing timestamp from an arbitrary fixed epoch.
    fn now(&self) -> Duration;

    /// Waits for at most `duration`.
    fn wait(&mut self, duration: Duration);
}

/// An error produced while initializing the adapter and constructing the Slint component.
#[derive(Debug)]
pub enum AdapterError<DisplayError, ResetError> {
    /// `mipidsi` failed to initialize the display.
    Display(mipidsi::InitError<DisplayError, ResetError>),
    /// A Slint platform has already been installed.
    Platform(platform::SetPlatformError),
    /// Construction of the generated Slint component failed.
    App(PlatformError),
}

impl<DisplayError: fmt::Debug, ResetError: fmt::Debug> fmt::Display
    for AdapterError<DisplayError, ResetError>
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Display(error) => write!(formatter, "display initialization failed: {error:?}"),
            Self::Platform(error) => write!(formatter, "could not install Slint platform: {error}"),
            Self::App(error) => write!(formatter, "could not create Slint app: {error}"),
        }
    }
}

impl<DisplayError: fmt::Debug, ResetError: fmt::Debug> core::error::Error
    for AdapterError<DisplayError, ResetError>
{
}

/// Result type returned by [`AdapterBuilder::build_with_event_loop`].
pub type BuildWithEventLoopResult<App, DI, MODEL, RST, TOUCH, RUNTIME> = Result<
    (
        App,
        McuEventLoop<mipidsi::Display<DI, MODEL, RST>, TOUCH, RUNTIME>,
    ),
    AdapterError<<DI as Interface>::Error, <RST as embedded_hal::digital::ErrorType>::Error>,
>;

/// Builds and installs the Slint MCU platform.
///
/// The builder owns an uninitialized but fully configured `mipidsi` builder, a touch
/// implementation, and the MCU runtime. Calling [`Self::build`] initializes the display,
/// installs the global Slint platform, constructs the generated component, and returns
/// that component unchanged.
pub struct AdapterBuilder<DI, MODEL, RST, TOUCH, RUNTIME>
where
    DI: Interface,
    MODEL: Model,
    MODEL::ColorFormat: InterfacePixelFormat<DI::Word>,
{
    display: mipidsi::Builder<DI, MODEL, RST>,
    touch: TOUCH,
    runtime: RUNTIME,
    poll_interval: Duration,
}

impl<DI, MODEL, RST, TOUCH, RUNTIME> AdapterBuilder<DI, MODEL, RST, TOUCH, RUNTIME>
where
    DI: Interface,
    MODEL: Model,
    MODEL::ColorFormat: InterfacePixelFormat<DI::Word>,
{
    /// Creates an adapter from a configured `mipidsi` builder, touch input, and runtime.
    ///
    /// The default event-loop polling interval is approximately one 60 Hz frame
    /// (`16_667` microseconds).
    #[must_use]
    pub const fn new(
        display: mipidsi::Builder<DI, MODEL, RST>,
        touch: TOUCH,
        runtime: RUNTIME,
    ) -> Self {
        Self {
            display,
            touch,
            runtime,
            poll_interval: Duration::from_micros(16_667),
        }
    }

    /// Sets the maximum interval between touch polls and animation frames.
    ///
    /// Slint timer deadlines can shorten this interval. A zero duration creates a busy
    /// event loop.
    #[must_use]
    pub const fn poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }
}

impl<DI, MODEL, RST, TOUCH, RUNTIME> AdapterBuilder<DI, MODEL, RST, TOUCH, RUNTIME>
where
    DI: Interface + 'static,
    DI::Error: fmt::Debug + 'static,
    MODEL: Model + 'static,
    MODEL::ColorFormat: InterfacePixelFormat<DI::Word> + From<Rgb565> + Copy + 'static,
    RST: OutputPin + 'static,
    TOUCH: TouchInput + 'static,
    TOUCH::Error: fmt::Debug + 'static,
    RUNTIME: McuRuntime + 'static,
{
    /// Initializes the hardware, installs the platform, and returns a normal Slint app.
    ///
    /// Pass the generated component's constructor as `app_factory`, for example
    /// `AppWindow::new`. The returned value is the generated component itself, so its
    /// properties, callbacks, [`slint::ComponentHandle::show`], and
    /// [`slint::ComponentHandle::run`] work as usual.
    ///
    /// Slint supports a single platform per program. Consequently this method may only
    /// succeed once.
    pub fn build<App>(
        self,
        delay: &mut impl DelayNs,
        app_factory: impl FnOnce() -> Result<App, PlatformError>,
    ) -> Result<App, AdapterError<DI::Error, RST::Error>> {
        self.build_with_event_loop(delay, app_factory)
            .map(|(app, _event_loop)| app)
    }

    /// Initializes the hardware and returns the Slint app together with a step-based event loop.
    ///
    /// This variant is intended for cooperative async executors such as Embassy. Call
    /// [`slint::ComponentHandle::show`] on the returned component and then repeatedly call
    /// [`McuEventLoop::step`], sleeping asynchronously for the returned duration between calls.
    /// Unlike [`Self::build`], this does not require calling the blocking
    /// [`slint::ComponentHandle::run`] method.
    ///
    /// Slint supports a single platform per program. Consequently this method may only
    /// succeed once.
    pub fn build_with_event_loop<App>(
        self,
        delay: &mut impl DelayNs,
        app_factory: impl FnOnce() -> Result<App, PlatformError>,
    ) -> BuildWithEventLoopResult<App, DI, MODEL, RST, TOUCH, RUNTIME> {
        let display = self.display.init(delay).map_err(AdapterError::Display)?;
        let display_size = display.size();
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        let line_buffer = vec![Rgb565Pixel::default(); display_size.width as usize];
        let started_at = self.runtime.now();

        let backend = Rc::new(McuBackend {
            window: window.clone(),
            display: RefCell::new(DisplayState {
                display,
                line_buffer,
            }),
            touch: RefCell::new(TouchTracker::new(
                self.touch,
                display_size.width,
                display_size.height,
            )),
            runtime: RefCell::new(self.runtime),
            started_at,
            poll_interval: self.poll_interval,
        });

        platform::set_platform(Box::new(McuPlatform {
            backend: backend.clone(),
        }))
        .map_err(AdapterError::Platform)?;
        let app = app_factory().map_err(AdapterError::App)?;
        window.set_size(PhysicalSize::new(display_size.width, display_size.height));

        Ok((app, McuEventLoop { backend }))
    }
}

/// Builds a Slint MCU platform for a display without touch input.
///
/// This is the display-only counterpart of [`AdapterBuilder`]. It installs
/// [`NoTouch`] internally, so callers only need to provide a configured
/// `mipidsi` builder and an [`McuRuntime`].
pub struct AdapterBuilderWithoutTouch<DI, MODEL, RST, RUNTIME>
where
    DI: Interface,
    MODEL: Model,
    MODEL::ColorFormat: InterfacePixelFormat<DI::Word>,
{
    inner: AdapterBuilder<DI, MODEL, RST, NoTouch, RUNTIME>,
}

impl<DI, MODEL, RST, RUNTIME> AdapterBuilderWithoutTouch<DI, MODEL, RST, RUNTIME>
where
    DI: Interface,
    MODEL: Model,
    MODEL::ColorFormat: InterfacePixelFormat<DI::Word>,
{
    /// Creates a display-only adapter from a configured `mipidsi` builder and runtime.
    ///
    /// The default event-loop polling interval is approximately one 60 Hz frame
    /// (`16_667` microseconds).
    #[must_use]
    pub const fn new(display: mipidsi::Builder<DI, MODEL, RST>, runtime: RUNTIME) -> Self {
        Self {
            inner: AdapterBuilder::new(display, NoTouch, runtime),
        }
    }

    /// Sets the maximum interval between event-loop iterations.
    ///
    /// Slint timer deadlines can shorten this interval. A zero duration creates
    /// a busy event loop.
    #[must_use]
    pub fn poll_interval(mut self, interval: Duration) -> Self {
        self.inner = self.inner.poll_interval(interval);
        self
    }
}

impl<DI, MODEL, RST, RUNTIME> AdapterBuilderWithoutTouch<DI, MODEL, RST, RUNTIME>
where
    DI: Interface + 'static,
    DI::Error: fmt::Debug + 'static,
    MODEL: Model + 'static,
    MODEL::ColorFormat: InterfacePixelFormat<DI::Word> + From<Rgb565> + Copy + 'static,
    RST: OutputPin + 'static,
    RUNTIME: McuRuntime + 'static,
{
    /// Initializes the hardware, installs the platform, and returns a normal Slint app.
    ///
    /// The returned value is the generated component itself. Use this method
    /// with [`slint::ComponentHandle::run`] for a blocking event loop.
    pub fn build<App>(
        self,
        delay: &mut impl DelayNs,
        app_factory: impl FnOnce() -> Result<App, PlatformError>,
    ) -> Result<App, AdapterError<DI::Error, RST::Error>> {
        self.inner.build(delay, app_factory)
    }

    /// Initializes the hardware and returns the Slint app with a cooperative event loop.
    ///
    /// Use this method with Embassy or another async executor. Show the app,
    /// repeatedly call [`McuEventLoop::step`], and wait asynchronously for the
    /// returned duration.
    pub fn build_with_event_loop<App>(
        self,
        delay: &mut impl DelayNs,
        app_factory: impl FnOnce() -> Result<App, PlatformError>,
    ) -> BuildWithEventLoopResult<App, DI, MODEL, RST, NoTouch, RUNTIME> {
        self.inner.build_with_event_loop(delay, app_factory)
    }
}

struct DisplayState<DISPLAY> {
    display: DISPLAY,
    line_buffer: Vec<Rgb565Pixel>,
}

struct McuBackend<DISPLAY, TOUCH, RUNTIME> {
    window: Rc<MinimalSoftwareWindow>,
    display: RefCell<DisplayState<DISPLAY>>,
    touch: RefCell<TouchTracker<TOUCH>>,
    runtime: RefCell<RUNTIME>,
    started_at: Duration,
    poll_interval: Duration,
}

/// A cooperative, step-based Slint event loop.
///
/// It is returned by [`AdapterBuilder::build_with_event_loop`] for integration with async
/// executors. The handle is deliberately single-threaded, matching Slint's
/// `unsafe-single-threaded` MCU configuration.
pub struct McuEventLoop<DISPLAY, TOUCH, RUNTIME> {
    backend: Rc<McuBackend<DISPLAY, TOUCH, RUNTIME>>,
}

impl<DISPLAY, TOUCH, RUNTIME> McuEventLoop<DISPLAY, TOUCH, RUNTIME>
where
    DISPLAY: DrawTarget + OriginDimensions,
    DISPLAY::Color: From<Rgb565>,
    DISPLAY::Error: fmt::Debug,
    TOUCH: TouchInput,
    TOUCH::Error: fmt::Debug,
    RUNTIME: McuRuntime,
{
    /// Processes timers, one touch sample, and any pending rendering.
    ///
    /// The returned duration is the maximum time the caller should wait before invoking
    /// this method again. Waking earlier is always valid.
    pub fn step(&self) -> Result<Duration, PlatformError> {
        platform::update_timers_and_animations();
        self.backend.dispatch_touch()?;
        self.backend.render()?;
        Ok(self.backend.next_wait())
    }

    /// Returns whether the Slint window is currently visible.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.backend.window.is_visible()
    }
}

struct McuPlatform<DISPLAY, TOUCH, RUNTIME> {
    backend: Rc<McuBackend<DISPLAY, TOUCH, RUNTIME>>,
}

impl<DISPLAY, TOUCH, RUNTIME> McuBackend<DISPLAY, TOUCH, RUNTIME>
where
    DISPLAY: DrawTarget + OriginDimensions,
    DISPLAY::Color: From<Rgb565>,
    DISPLAY::Error: fmt::Debug,
    TOUCH: TouchInput,
    TOUCH::Error: fmt::Debug,
    RUNTIME: McuRuntime,
{
    fn dispatch_touch(&self) -> Result<(), PlatformError> {
        let update = self
            .touch
            .borrow_mut()
            .poll()
            .map_err(|error| PlatformError::from(format!("touch input failed: {error:?}")))?;

        let button = PointerEventButton::Left;
        let event = match update {
            TouchUpdate::None => return Ok(()),
            TouchUpdate::Pressed(position) => WindowEvent::PointerPressed { position, button },
            TouchUpdate::Moved(position) => WindowEvent::PointerMoved { position },
            TouchUpdate::Released(position) => WindowEvent::PointerReleased { position, button },
        };
        let released = matches!(event, WindowEvent::PointerReleased { .. });
        self.window.try_dispatch_event(event)?;
        if released {
            self.window.try_dispatch_event(WindowEvent::PointerExited)?;
        }
        Ok(())
    }

    fn render(&self) -> Result<(), PlatformError> {
        let mut display_error = None;
        self.window.draw_if_needed(|renderer| {
            let mut state = self.display.borrow_mut();
            let DisplayState {
                display,
                line_buffer,
            } = &mut *state;

            renderer.render_by_line(MipidsiLineBuffer {
                display,
                line_buffer,
                error: &mut display_error,
            });
        });

        display_error
            .map(|error| PlatformError::from(format!("display rendering failed: {error:?}")))
            .map_or(Ok(()), Err)
    }

    fn next_wait(&self) -> Duration {
        platform::duration_until_next_timer_update()
            .map(|duration| duration.min(self.poll_interval))
            .unwrap_or(self.poll_interval)
    }
}

impl<DISPLAY, TOUCH, RUNTIME> Platform for McuPlatform<DISPLAY, TOUCH, RUNTIME>
where
    DISPLAY: DrawTarget + OriginDimensions + 'static,
    DISPLAY::Color: From<Rgb565> + 'static,
    DISPLAY::Error: fmt::Debug + 'static,
    TOUCH: TouchInput + 'static,
    TOUCH::Error: fmt::Debug + 'static,
    RUNTIME: McuRuntime + 'static,
{
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.backend.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.backend
            .runtime
            .borrow()
            .now()
            .saturating_sub(self.backend.started_at)
    }

    fn run_event_loop(&self) -> Result<(), PlatformError> {
        while self.backend.window.is_visible() {
            platform::update_timers_and_animations();
            self.backend.dispatch_touch()?;
            self.backend.render()?;

            let wait = self.backend.next_wait();
            if !wait.is_zero() {
                self.backend.runtime.borrow_mut().wait(wait);
            }
        }
        Ok(())
    }
}

struct MipidsiLineBuffer<'a, DISPLAY>
where
    DISPLAY: DrawTarget,
{
    display: &'a mut DISPLAY,
    line_buffer: &'a mut [Rgb565Pixel],
    error: &'a mut Option<DISPLAY::Error>,
}

impl<DISPLAY> LineBufferProvider for MipidsiLineBuffer<'_, DISPLAY>
where
    DISPLAY: DrawTarget,
    DISPLAY::Color: From<Rgb565>,
{
    type TargetPixel = Rgb565Pixel;

    fn process_line(
        &mut self,
        line: usize,
        range: core::ops::Range<usize>,
        render_fn: impl FnOnce(&mut [Self::TargetPixel]),
    ) {
        render_fn(&mut self.line_buffer[range.clone()]);

        if self.error.is_some() {
            return;
        }

        let area = Rectangle::new(
            Point::new(range.start as i32, line as i32),
            Size::new(range.len() as u32, 1),
        );
        let colors = self.line_buffer[range].iter().map(|pixel| {
            let rgb565: Rgb565 = RawU16::new(pixel.0).into();
            rgb565.into()
        });
        if let Err(error) = self.display.fill_contiguous(&area, colors) {
            *self.error = Some(error);
        }
    }
}

struct TouchTracker<T> {
    touch: T,
    last: Option<LogicalPosition>,
    max_x: u16,
    max_y: u16,
}

impl<T> TouchTracker<T> {
    fn new(touch: T, width: u32, height: u32) -> Self {
        Self {
            touch,
            last: None,
            max_x: width.saturating_sub(1).min(u16::MAX.into()) as u16,
            max_y: height.saturating_sub(1).min(u16::MAX.into()) as u16,
        }
    }
}

impl<T: TouchInput> TouchTracker<T> {
    fn poll(&mut self) -> Result<TouchUpdate, T::Error> {
        let current = self.read_position()?;
        Ok(match (self.last, current) {
            (None, None) => TouchUpdate::None,
            (None, Some(position)) => {
                self.last = Some(position);
                TouchUpdate::Pressed(position)
            }
            (Some(previous), None) => {
                self.last = None;
                TouchUpdate::Released(previous)
            }
            (Some(previous), Some(position)) if previous == position => TouchUpdate::None,
            (Some(_), Some(position)) => {
                self.last = Some(position);
                TouchUpdate::Moved(position)
            }
        })
    }

    fn read_position(&mut self) -> Result<Option<LogicalPosition>, T::Error> {
        self.touch.read_touch().map(|point| {
            point.map(|point| {
                LogicalPosition::new(
                    point.x.min(self.max_x) as f32,
                    point.y.min(self.max_y) as f32,
                )
            })
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum TouchUpdate {
    None,
    Pressed(LogicalPosition),
    Moved(LogicalPosition),
    Released(LogicalPosition),
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::collections::VecDeque;

    use embedded_graphics_core::{Pixel, pixelcolor::RgbColor};

    use super::*;

    struct FakeTouch {
        samples: VecDeque<Option<TouchPoint>>,
    }

    impl TouchInput for FakeTouch {
        type Error = core::convert::Infallible;

        fn read_touch(&mut self) -> Result<Option<TouchPoint>, Self::Error> {
            Ok(self.samples.pop_front().flatten())
        }
    }

    #[test]
    fn touch_samples_become_pointer_transitions() {
        let touch = FakeTouch {
            samples: [
                None,
                Some(TouchPoint::new(10, 20)),
                Some(TouchPoint::new(11, 21)),
                Some(TouchPoint::new(11, 21)),
                None,
            ]
            .into(),
        };
        let mut tracker = TouchTracker::new(touch, 320, 240);

        assert_eq!(tracker.poll().unwrap(), TouchUpdate::None);
        assert_eq!(
            tracker.poll().unwrap(),
            TouchUpdate::Pressed(LogicalPosition::new(10.0, 20.0))
        );
        assert_eq!(
            tracker.poll().unwrap(),
            TouchUpdate::Moved(LogicalPosition::new(11.0, 21.0))
        );
        assert_eq!(tracker.poll().unwrap(), TouchUpdate::None);
        assert_eq!(
            tracker.poll().unwrap(),
            TouchUpdate::Released(LogicalPosition::new(11.0, 21.0))
        );
    }

    #[test]
    fn touch_coordinates_are_clamped_to_the_display() {
        let touch = FakeTouch {
            samples: [Some(TouchPoint::new(u16::MAX, u16::MAX))].into(),
        };
        let mut tracker = TouchTracker::new(touch, 320, 240);

        assert_eq!(
            tracker.poll().unwrap(),
            TouchUpdate::Pressed(LogicalPosition::new(319.0, 239.0))
        );
    }

    #[test]
    fn public_types_are_send_free_and_no_std_friendly() {
        static_assertions::assert_impl_all!(TouchPoint: Copy, Send, Sync);
        static_assertions::assert_impl_all!(NoTouch: Copy, Send, Sync);
    }

    #[derive(Default)]
    struct RecordingDisplay {
        pixels: Vec<Pixel<Rgb565>>,
    }

    impl OriginDimensions for RecordingDisplay {
        fn size(&self) -> Size {
            Size::new(4, 4)
        }
    }

    impl DrawTarget for RecordingDisplay {
        type Color = Rgb565;
        type Error = core::convert::Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            self.pixels.extend(pixels);
            Ok(())
        }
    }

    #[test]
    fn line_renderer_writes_only_the_dirty_range() {
        let mut display = RecordingDisplay::default();
        let mut buffer = [Rgb565Pixel::default(); 4];
        let mut error = None;
        let mut provider = MipidsiLineBuffer {
            display: &mut display,
            line_buffer: &mut buffer,
            error: &mut error,
        };

        provider.process_line(3, 1..3, |line| {
            line[0] = Rgb565Pixel(0xf800);
            line[1] = Rgb565Pixel(0x07e0);
        });

        assert!(error.is_none());
        assert_eq!(
            display.pixels,
            [
                Pixel(Point::new(1, 3), Rgb565::RED),
                Pixel(Point::new(2, 3), Rgb565::GREEN),
            ]
        );
    }
}
