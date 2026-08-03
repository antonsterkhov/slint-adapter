# slint-mipidsi-adapter

**English** | [Русская версия](README.ru.md)

`slint-mipidsi-adapter` is a small `#![no_std]` Slint platform adapter for
microcontrollers. It connects:

- a configured, but not yet initialized, `mipidsi::Builder`;
- a touch input implementation through `TouchInput`;
- a monotonic clock and waiting implementation through `McuRuntime`;
- Slint's software renderer.

After initialization, the adapter returns the generated Slint component without
wrapping it. You can use it like a regular Slint application: set properties,
register callbacks, and call `show()` or `run()`.

The crate targets Slint 1.17, `mipidsi` 0.10, and `embedded-hal` 1.0.

## What the adapter does

- Accepts a ready-to-use `mipidsi::Builder` with the selected display model,
  interface, orientation, and reset pin.
- Calls `mipidsi::Builder::init`.
- Installs a global `slint::platform::Platform` implementation.
- Renders the UI line by line in RGB565 format.
- Stores only one pixel line, approximately `width × 2` bytes, instead of a
  full-screen framebuffer.
- Converts a sequence of touch states into Slint pointer events.
- Supports both a blocking event loop and a step-based event loop for Embassy or
  another cooperative executor.

The adapter does not configure SPI/I²C, bus frequencies, power rails,
backlight, touch calibration, or the global allocator. These parts depend on
the target board and remain the responsibility of the application's BSP.

## Dependency setup

```toml
[dependencies]
slint-mipidsi-adapter = "0.2.0"

slint = { version = "1.17.1", default-features = false, features = [
    "compat-1-2",
    "unsafe-single-threaded",
    "libm",
    "renderer-software",
] }

[build-dependencies]
slint-build = "1.17.1"
```

Slint requires a global allocator on bare metal. Install and initialize it
before constructing the Slint component.

Prepare UI resources for the software renderer:

```rust,ignore
fn main() {
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new()
            .embed_resources(
                slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer,
            ),
    )
    .unwrap();
}
```

## Basic usage

The application first configures the board's power, GPIO, SPI, I²C, and device
controllers. It then passes a configured `mipidsi::Builder`, touch input, and
runtime to `AdapterBuilder`.

```rust,ignore
#![no_std]
#![no_main]

extern crate alloc;

use core::time::Duration;
use slint::ComponentHandle;
use slint_mipidsi_adapter::{
    AdapterBuilder, McuRuntime, TouchInput, TouchPoint,
};

slint::include_modules!();

struct BoardTouch {
    // I²C touch driver, calibration state, and so on.
}

impl TouchInput for BoardTouch {
    type Error = TouchError;

    fn read_touch(&mut self) -> Result<Option<TouchPoint>, Self::Error> {
        let sample = self.driver.scan()?;

        Ok(sample.map(|sample| {
            // Apply calibration and orientation conversion here.
            TouchPoint::new(sample.x, sample.y)
        }))
    }
}

struct BoardRuntime {
    // The board's monotonic timer.
}

impl McuRuntime for BoardRuntime {
    fn now(&self) -> Duration {
        Duration::from_micros(self.timer.now_micros())
    }

    fn wait(&mut self, duration: Duration) {
        self.timer.arm(duration);
        cortex_m::asm::wfi();
    }
}

#[entry]
fn main() -> ! {
    // 1. Initialize the allocator, power, GPIO, SPI, and I²C.
    // 2. Enable display power and backlight.
    // 3. Hardware-reset the display and touch controller if required.

    let display_builder = mipidsi::Builder::new(
        mipidsi::models::ST7789,
        display_interface,
    )
    .reset_pin(display_reset)
    .display_size(240, 320)
    .orientation(
        mipidsi::options::Orientation::new()
            .rotate(mipidsi::options::Rotation::Deg90),
    );

    let app = AdapterBuilder::new(display_builder, board_touch, board_runtime)
        .build(&mut delay, AppWindow::new)
        .unwrap();

    // The returned value is the actual generated Slint component.
    app.set_counter(42);
    app.on_increment({
        let app = app.as_weak();
        move || {
            if let Some(app) = app.upgrade() {
                app.set_counter(app.get_counter() + 1);
            }
        }
    });

    app.run().unwrap();

    loop {
        cortex_m::asm::wfi();
    }
}
```

## Using Embassy

Do not call the blocking `app.run()` from an Embassy task: it would never yield
control back to the executor. Use `build_with_event_loop` in async
applications.

```rust,ignore
let (app, event_loop) =
    AdapterBuilder::new(display_builder, touch, runtime)
        .poll_interval(core::time::Duration::from_millis(16))
        .build_with_event_loop(&mut delay, AppWindow::new)?;

app.show()?;

while event_loop.is_running() {
    let wait = event_loop.step()?;
    let micros = wait.as_micros().min(u128::from(u64::MAX)) as u64;

    if micros == 0 {
        embassy_time::Timer::after_ticks(1).await;
    } else {
        embassy_time::Timer::after_micros(micros).await;
    }
}
```

One `McuEventLoop::step` call:

1. updates Slint timers and animations;
2. reads one current touch state;
3. dispatches a pointer event to the window;
4. renders pending changes;
5. returns the maximum duration until the next call.

A complete ESP32 implementation is available in
[`examples/m5stack-core2`](https://github.com/antonsterkhov/slint-mipidsi-adapter/tree/main/examples/m5stack-core2).

## Display without touch

Use `AdapterBuilderWithoutTouch` when the product has no touch controller or
when input is handled entirely through another mechanism:

```rust,ignore
use slint_mipidsi_adapter::AdapterBuilderWithoutTouch;

let app = AdapterBuilderWithoutTouch::new(display_builder, runtime)
    .build(&mut delay, AppWindow::new)?;

app.run()?;
```

The same builder supports a cooperative event loop:

```rust,ignore
let (app, event_loop) =
    AdapterBuilderWithoutTouch::new(display_builder, runtime)
        .poll_interval(core::time::Duration::from_millis(50))
        .build_with_event_loop(&mut delay, AppWindow::new)?;

app.show()?;

while event_loop.is_running() {
    let wait = event_loop.step()?;
    embassy_time::Timer::after_micros(wait.as_micros() as u64).await;
}
```

`AdapterBuilderWithoutTouch` installs [`NoTouch`] internally. The application
does not need a dummy `TouchInput` implementation. Slint timers, animations,
property updates, callbacks invoked by application code, and rendering continue
to work normally; the platform simply does not generate pointer events.

# Touch input in detail

## The `TouchInput` contract

`TouchInput` is a synchronous polling interface:

```rust,ignore
pub trait TouchInput {
    type Error;

    fn read_touch(&mut self)
        -> Result<Option<TouchPoint>, Self::Error>;
}
```

The method must return a **snapshot of the current primary contact**, not a
queue of one-shot events:

- `Ok(Some(point))`: a finger is currently touching the screen;
- `Ok(None)`: there is currently no active contact;
- `Err(error)`: the controller or bus could not complete the read.

While a finger remains down, the implementation must return `Some(point)` on
every poll. Returning a coordinate only for the initial IRQ and then returning
`None` would be interpreted as an immediate release.

## How states become Slint events

The adapter remembers the previous state and produces events automatically:

| Previous state | New state | Slint event |
|---|---|---|
| `None` | `None` | no event |
| `None` | `Some(point)` | `PointerPressed` |
| `Some(old)` | `Some(new)`, coordinates changed | `PointerMoved` |
| `Some(point)` | the same point | no event |
| `Some(old)` | `None` | `PointerReleased`, then `PointerExited` |

Standard Slint controls such as `Button`, `TouchArea`, and `Slider` therefore
work without special code in the `.slint` file:

```slint
import { Button } from "std-widgets.slint";

export component AppWindow inherits Window {
    in-out property <int> counter: 0;

    Button {
        text: "Press";
        clicked => {
            root.counter += 1;
        }
    }
}
```

The current adapter uses one primary contact and
`PointerEventButton::Left`. If the controller supports multitouch, the
`TouchInput` implementation must select one contact, usually the first active
one.

## Minimal implementation

If the driver already provides calibrated screen coordinates, the adapter can
be as small as:

```rust,ignore
struct MyTouch<DRIVER> {
    driver: DRIVER,
}

impl<DRIVER> TouchInput for MyTouch<DRIVER>
where
    DRIVER: TouchDriver,
{
    type Error = DRIVER::Error;

    fn read_touch(&mut self) -> Result<Option<TouchPoint>, Self::Error> {
        let data = self.driver.read_primary_touch()?;

        Ok(data.map(|point| TouchPoint::new(point.x, point.y)))
    }
}
```

For devices without a touch panel, prefer `AdapterBuilderWithoutTouch`. The
lower-level `NoTouch` implementation remains available when generic code needs
to construct `AdapterBuilder` directly.

## FT6336U example

`ft6336u-driver` reports up to two points. For Slint, select the first point
that is not in the `Release` state:

```rust,ignore
use embedded_hal::i2c::I2c;
use ft6336u_driver::{
    Error, FT6336U, GestureMode, TouchStatus,
};
use slint_mipidsi_adapter::{TouchInput, TouchPoint};

struct Ft6336Touch<I2C> {
    controller: FT6336U<I2C>,
}

impl<I2C> Ft6336Touch<I2C>
where
    I2C: I2c,
{
    fn new(i2c: I2C) -> Result<Self, Error<I2C::Error>> {
        let mut controller = FT6336U::new(i2c);

        // The adapter polls regularly, making polling mode the simplest and
        // most reliable option.
        controller.write_g_mode(GestureMode::Polling)?;

        Ok(Self { controller })
    }
}

impl<I2C> TouchInput for Ft6336Touch<I2C>
where
    I2C: I2c,
{
    type Error = Error<I2C::Error>;

    fn read_touch(&mut self) -> Result<Option<TouchPoint>, Self::Error> {
        let data = self.controller.scan()?;

        Ok(data
            .points
            .iter()
            .find(|point| point.status != TouchStatus::Release)
            .map(|point| TouchPoint::new(point.x, point.y)))
    }
}
```

If the PMIC, touch controller, RTC, and other devices share one I²C bus, give
each driver a virtual device from `embedded-hal-bus`:

```rust,ignore
use core::cell::RefCell;
use embedded_hal_bus::i2c::RefCellDevice;

let bus = RefCell::new(i2c);

let touch_i2c = RefCellDevice::new(&bus);
let pmic_i2c = RefCellDevice::new(&bus);

let touch = Ft6336Touch::new(touch_i2c)?;
let power = Axp192::new(pmic_i2c);
```

`RefCellDevice` is suitable only for single-threaded access. This matches
Slint's `unsafe-single-threaded` mode when all access occurs in one task and
never from an interrupt handler.

## Coordinates, rotation, and mirroring

`TouchPoint` must use the **logical display coordinate system after applying
the `mipidsi::Builder` configuration**:

- `(0, 0)` is the top-left corner of the Slint window;
- `x` increases to the right;
- `y` increases downwards;
- maximum values are normally `width - 1` and `height - 1`.

Rotating the LCD with `mipidsi::Builder::orientation` does not necessarily
rotate the touch controller's raw coordinates. The BSP or `TouchInput`
implementation must apply the same transform.

For a raw panel of size `W × H`, typical transforms are:

| Orientation | Logical coordinate |
|---|---|
| 0° | `(x, y)` |
| 90° clockwise | `(H - 1 - y, x)` |
| 180° | `(W - 1 - x, H - 1 - y)` |
| 270° clockwise | `(y, W - 1 - x)` |

A particular controller may already swap axes or invert one of them. Verify
the conversion against all four corners of the physical display.

The adapter clamps final coordinates to the display bounds. This protects
against isolated outliers, but does not replace calibration.

## Calibrating raw coordinates

If a controller reports values in a `raw_min..raw_max` range, scale them to the
Slint window size:

```rust,ignore
fn scale_axis(raw: u16, raw_min: u16, raw_max: u16, screen_size: u16) -> u16 {
    let raw = raw.clamp(raw_min, raw_max);
    let input = u32::from(raw - raw_min);
    let input_range = u32::from(raw_max - raw_min).max(1);
    let output_range = u32::from(screen_size.saturating_sub(1));

    (input * output_range / input_range) as u16
}
```

For a more accurate resistive or non-linear panel, apply an affine transform
before creating the `TouchPoint`.

Some panels have an active area larger than the LCD. For example, the
M5Stack Core2 FT6336U reports a `320×280` area: the bottom 40 rows belong to
three capacitive zones below the display. If the UI does not use them, discard
such samples:

```rust,ignore
if raw_y >= DISPLAY_HEIGHT {
    return Ok(None);
}
```

Do not rely on automatic clamping in this case. Otherwise, a touch below the
screen would become a false touch on its bottom edge.

## Polling and interrupts

By default, the adapter polls touch no less frequently than `poll_interval`.
The default value is `16_667` microseconds, approximately 60 Hz:

```rust,ignore
let builder = AdapterBuilder::new(display, touch, runtime)
    .poll_interval(core::time::Duration::from_millis(10));
```

A shorter interval reduces input latency but increases I²C traffic and CPU
active time.

An IRQ may wake the MCU or set a "controller needs reading" flag. However:

- never call Slint APIs directly from an interrupt handler;
- perform I²C reads in a regular task;
- after the initial IRQ, keep returning `Some(point)` while the finger remains
  down;
- the release must still produce one `Ok(None)` sample.

If the controller's IRQ is a short pulse, merely checking the current GPIO
level in `read_touch()` can miss events. Latch a flag from the IRQ or configure
the controller for polling mode.

## Read errors

A `TouchInput::Error` is converted into `slint::PlatformError`.

- With blocking `app.run()`, the event loop exits with an error.
- With manual `event_loop.step()`, the current step returns the error.

For robust products, distinguish temporary I²C failures from permanent
failures. A BSP may retry, recover the bus, or skip one sample. Avoid converting
every temporary failure to `Ok(None)`: during a hold, this produces a false
`PointerReleased` followed by another `PointerPressed`.

## Testing a touch implementation

Before connecting a complex UI, verify:

1. touches in all four corners;
2. horizontal and vertical movement;
3. a stationary hold without false releases;
4. a correct release;
5. no events outside the LCD area;
6. operation after wake-up and I²C reinitialization;
7. coordinates matching the selected `mipidsi` rotation.

A standard Slint button should then press, display its held state, and emit
`clicked` only after a correct press/release sequence.

## The `McuRuntime` contract

`McuRuntime::now` must return monotonic time. Slint uses it for timers and
animations.

`McuRuntime::wait` receives the smaller of:

- the duration until the nearest Slint timer;
- the configured `poll_interval`.

It may return early, for example after a touch IRQ. In a step-based Embassy
event loop, the async task performs the wait itself. The runtime still remains
part of the installed platform and allows ordinary `app.run()` to work.

All Slint calls must stay on one thread/task and must not run from an interrupt
handler.

## Display and memory

The line renderer uses `Rgb565Pixel` and calls `DrawTarget::fill_contiguous`
for the changed range of each line.

The `Rgb565` and `Rgb666` formats exposed by `mipidsi` 0.10 are supported
through conversion from `embedded_graphics_core::pixelcolor::Rgb565`.

The adapter does not use DMA and does not own the SPI configuration. Tune the
SPI frequency, display-interface buffer size, and DMA in the target MCU's BSP.

## References

- [Slint on microcontrollers](https://docs.slint.dev/latest/docs/rust/slint/docs/mcu/)
- [Slint `Platform`](https://docs.slint.dev/latest/docs/rust/slint/platform/trait.Platform)
- [Slint `LineBufferProvider`](https://docs.slint.dev/latest/docs/rust/slint/platform/software_renderer/trait.LineBufferProvider)
- [`mipidsi::Builder`](https://docs.rs/mipidsi/0.10.0/mipidsi/struct.Builder.html)
- [`mipidsi::Display`](https://docs.rs/mipidsi/0.10.0/mipidsi/struct.Display.html)
- [M5Stack Core2 hardware example](https://github.com/antonsterkhov/slint-mipidsi-adapter/tree/main/examples/m5stack-core2)
