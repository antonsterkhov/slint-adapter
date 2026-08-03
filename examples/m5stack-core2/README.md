# M5Stack Core2: Slint + esp-hal + Embassy

**English** | [Русская версия](README.ru.md)

This is a hardware example for the original ESP32/AXP192-based M5Stack Core2.
The project was generated with `esp-generate 1.3.0` and uses the following
bare-metal `no_std` stack:

- `esp-hal 1.1` for GPIO, I²C, SPI, and timers;
- `esp-rtos 0.3` for Embassy integration;
- `mipidsi 0.10` with `ILI9342CRgb565` for the LCD;
- `ft6336u-driver 1.0` for the touch controller;
- `axp192 0.2` for LCD power, backlight, and the shared reset line;
- the local `slint-mipidsi-adapter` crate and Slint's software renderer.

## Connected peripherals

| Device | Core2 connection |
|---|---|
| ILI9342C | SCLK GPIO18, MOSI GPIO23, CS GPIO5, D/C GPIO15 |
| FT6336U | I²C 0x38, SDA GPIO21, SCL GPIO22, INT GPIO39 |
| AXP192 | I²C 0x34, SDA GPIO21, SCL GPIO22 |
| LCD/touch reset | AXP192 GPIO4 |
| LCD/peripheral power | AXP192 LDO2, 3.3 V |
| Backlight | AXP192 DCDC3 |

The display's GPIO38/MISO connection is not used because the renderer only
writes pixels. Touch and PMIC drivers receive separate `RefCellDevice`
instances backed by the same physical I²C bus.

At startup, the AXP192 setup:

1. configures and enables the 3.3 V LDO2 LCD/peripheral rail;
2. sets a safe initial backlight level through DCDC3;
3. disables the vibration motor and speaker amplifier;
4. hardware-resets the LCD and touch controller through AXP192 GPIO4.

The implementation in `power.rs` also provides methods for changing backlight
brightness and enabling or disabling screen power.

## Build and flash

Install the Espressif Rust toolchain and `espflash`. From this example's
directory, run:

```text
cargo check
cargo run --release
```

The local `.cargo/config.toml` already selects
`xtensa-esp32-none-elf` and configures this runner:

```text
espflash flash --monitor --chip esp32
```

If more than one serial port is connected, `espflash` asks you to select the
target port. To flash a known port directly:

```text
espflash flash --chip esp32 --port COM11 \
    target/xtensa-esp32-none-elf/release/m5stack-core2
```

## Execution architecture

`main` configures power and hardware buses, starts Embassy, and moves the
display resources into `ui_task`.

The UI task:

1. initializes the ILI9342C from the configured `mipidsi::Builder`;
2. constructs the regular generated `AppWindow`;
3. shows the window;
4. performs one Slint event-loop step;
5. sleeps asynchronously until the nearest Slint timer or the next touch poll.

Rendering therefore does not block the Embassy executor indefinitely, and the
main task remains available for future application logic and peripherals.

## Touch behavior

The FT6336U is configured in polling mode. On each Slint event-loop step,
`Core2Touch::read_touch` scans the controller and selects the first active
contact.

The Core2 digitizer covers `320×280` pixels, including three capacitive zones
below the `320×240` LCD. This example forwards only points with `y < 240` to
Slint. Samples below the visible display are discarded instead of being
clamped to the bottom edge.

`slint-mipidsi-adapter` converts the sampled states into:

- first contact → `PointerPressed`;
- coordinate change → `PointerMoved`;
- no active contact after a hold → `PointerReleased`.

The demo contains two screens. On the home screen:

- `Add click` invokes a Slint callback handled in Rust and increments the
  counter;
- `Open details` invokes another Rust callback and switches to the details
  screen.

The details screen displays the same counter and its `Back to home` button
navigates back through a third Rust callback. This demonstrates that the app
returned by `slint-mipidsi-adapter` exposes the regular generated Slint API:
`on_*` callbacks and `get_*`/`set_*` properties.

## Board revision

This example targets the Core2 revision with an **AXP192**, matching the
official Core2 documentation. Core2 v1.1 uses an AXP2101 with a different power
configuration and requires a separate power backend.

## Related documentation

- [Main `slint-mipidsi-adapter` documentation](../../README.md)
- [Main documentation in Russian](../../README.ru.md)
- [M5Stack Core2 documentation](https://docs.m5stack.com/en/core/core2)
- [Rust on ESP: esp-generate](https://docs.espressif.com/projects/rust/book/getting-started/tooling/esp-generate.html)
- [Rust on ESP: async and Embassy](https://docs.espressif.com/projects/rust/book/application-development/async.html)
- [Slint on microcontrollers](https://docs.slint.dev/latest/docs/rust/slint/docs/mcu/)
