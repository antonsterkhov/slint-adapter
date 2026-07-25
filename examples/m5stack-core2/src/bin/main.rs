#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use embedded_hal_bus::i2c::RefCellDevice;
use esp_backtrace as _;
use esp_hal::{
    Blocking,
    clock::CpuClock,
    delay::Delay,
    gpio::{Input, InputConfig, Level, Output, OutputConfig},
    i2c::master::{Config as I2cConfig, I2c},
    spi::{
        Mode,
        master::{Config as SpiConfig, Spi},
    },
    time::Rate,
    timer::timg::TimerGroup,
};
use log::info;
use static_cell::StaticCell;

extern crate alloc;

use core::cell::RefCell;

#[path = "../power.rs"]
mod power;
#[path = "../touch.rs"]
mod touch;
#[path = "../ui.rs"]
mod ui;

type InternalI2c = I2c<'static, Blocking>;
type SharedI2cDevice = RefCellDevice<'static, InternalI2c>;

static INTERNAL_I2C: StaticCell<RefCell<InternalI2c>> = StaticCell::new();

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // generator version: 1.3.0
    // generator parameters: --chip esp32 -o unstable-hal -o alloc -o embassy -o esp-backtrace -o log

    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 98768);

    // Core2 internal I2C bus: AXP192 (0x34) + FT6336U (0x38).
    let i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .expect("I2C configuration failed")
    .with_sda(peripherals.GPIO21)
    .with_scl(peripherals.GPIO22);
    let i2c = INTERNAL_I2C.init(RefCell::new(i2c));

    let mut delay = Delay::new();
    let power = power::Core2Power::initialize(RefCellDevice::new(i2c), &mut delay)
        .expect("AXP192 initialization failed");
    info!("AXP192 configured");

    let touch_interrupt = Input::new(peripherals.GPIO39, InputConfig::default());
    let touch = touch::Core2Touch::initialize(RefCellDevice::new(i2c), touch_interrupt)
        .expect("FT6336U initialization failed");
    info!("FT6336U configured");

    // Core2 LCD SPI: SCLK=18, MOSI=23, CS=5, D/C=15. MISO=38 is not
    // connected because rendering is write-only.
    let spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(40))
            .with_mode(Mode::_0),
    )
    .expect("SPI configuration failed")
    .with_sck(peripherals.GPIO18)
    .with_mosi(peripherals.GPIO23);
    let lcd_cs = Output::new(peripherals.GPIO5, Level::High, OutputConfig::default());
    let lcd_dc = Output::new(peripherals.GPIO15, Level::Low, OutputConfig::default());

    let ui_hardware = ui::UiHardware {
        spi,
        lcd_cs,
        lcd_dc,
        touch,
        power,
    };

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("Embassy initialized");
    spawner.spawn(ui::ui_task(ui_hardware).expect("failed to allocate the static UI task"));

    loop {
        // All display rendering, touch polling and Slint timers run independently
        // in ui_task. This main task is now free for application work.
        Timer::after(Duration::from_secs(60)).await;
    }
}
