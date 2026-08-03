use core::time::Duration;

use embassy_time::{Instant, Timer};
use embedded_hal::delay::DelayNs;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::{Blocking, delay::Delay, gpio::Output, spi::master::Spi};
use mipidsi::{Builder, interface::SpiInterface, models::ILI9342CRgb565, options::ColorInversion};
use slint::ComponentHandle;
use slint_mipidsi_adapter::{AdapterBuilder, McuRuntime};
use static_cell::ConstStaticCell;

use crate::{SharedI2cDevice, power::Core2Power, touch::Core2Touch};

slint::include_modules!();

static DISPLAY_BUFFER: ConstStaticCell<[u8; 512]> = ConstStaticCell::new([0; 512]);

/// Hardware moved into the single-threaded Slint/Embassy task.
pub struct UiHardware {
    pub spi: Spi<'static, Blocking>,
    pub lcd_cs: Output<'static>,
    pub lcd_dc: Output<'static>,
    pub touch: Core2Touch<SharedI2cDevice>,
    pub power: Core2Power<SharedI2cDevice>,
}

#[derive(Clone, Copy)]
struct EmbassyRuntime;

impl McuRuntime for EmbassyRuntime {
    fn now(&self) -> Duration {
        Duration::from_micros(Instant::now().as_micros())
    }

    fn wait(&mut self, duration: Duration) {
        let micros = duration.as_micros().min(u128::from(u32::MAX)) as u32;
        Delay::new().delay_us(micros);
    }
}

#[embassy_executor::task]
pub async fn ui_task(hardware: UiHardware) {
    let UiHardware {
        spi,
        lcd_cs,
        lcd_dc,
        touch,
        power: _power,
    } = hardware;

    let spi_device = ExclusiveDevice::new(spi, lcd_cs, Delay::new()).expect("LCD CS setup failed");
    let interface = SpiInterface::new(spi_device, lcd_dc, DISPLAY_BUFFER.take().as_mut_slice());

    // M5GFX's Core2 profile uses the ILI9342C in its native 320x240
    // orientation with color inversion enabled.
    let display = Builder::new(ILI9342CRgb565, interface).invert_colors(ColorInversion::Inverted);

    let mut delay = Delay::new();
    let (app, event_loop) = AdapterBuilder::new(display, touch, EmbassyRuntime)
        .build_with_event_loop(&mut delay, AppWindow::new)
        .expect("Slint display platform initialization failed");

    app.on_increment_requested({
        let app = app.as_weak();
        move || {
            if let Some(app) = app.upgrade() {
                let counter = app.get_counter().saturating_add(1);
                app.set_counter(counter);
                log::info!("Home button clicked: counter={counter}");
            }
        }
    });

    app.on_details_requested({
        let app = app.as_weak();
        move || {
            if let Some(app) = app.upgrade() {
                app.set_show_details(true);
                log::info!("Navigated to details screen");
            }
        }
    });

    app.on_back_requested({
        let app = app.as_weak();
        move || {
            if let Some(app) = app.upgrade() {
                app.set_show_details(false);
                log::info!("Navigated back to home screen");
            }
        }
    });

    app.show().expect("failed to show Slint window");
    log::info!("Slint UI is running");

    while event_loop.is_running() {
        let wait = event_loop.step().expect("Slint event-loop step failed");
        if wait.is_zero() {
            Timer::after_ticks(1).await;
        } else {
            let micros = wait.as_micros().min(u128::from(u64::MAX)) as u64;
            Timer::after_micros(micros).await;
        }
    }
}
