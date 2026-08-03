use embedded_hal::i2c::I2c;
use esp_hal::gpio::Input;
use ft6336u_driver::{Error, FT6336U, GestureMode, TouchStatus};
use slint_mipidsi_adapter::{TouchInput, TouchPoint};

const DISPLAY_HEIGHT: u16 = 240;

/// FT6336U touch input adapted to Slint's primary-pointer model.
pub struct Core2Touch<I2C> {
    controller: FT6336U<I2C>,
    // GPIO39 is retained for future interrupt-driven wake-up. The adapter polls at
    // frame rate, so the controller is deliberately configured in polling mode.
    _interrupt: Input<'static>,
}

impl<I2C> Core2Touch<I2C>
where
    I2C: I2c,
{
    pub fn initialize(i2c: I2C, interrupt: Input<'static>) -> Result<Self, Error<I2C::Error>> {
        let mut controller = FT6336U::new(i2c);
        controller.write_g_mode(GestureMode::Polling)?;

        Ok(Self {
            controller,
            _interrupt: interrupt,
        })
    }
}

impl<I2C> TouchInput for Core2Touch<I2C>
where
    I2C: I2c,
{
    type Error = Error<I2C::Error>;

    fn read_touch(&mut self) -> Result<Option<TouchPoint>, Self::Error> {
        let data = self.controller.scan()?;
        let point = data
            .points
            .iter()
            .find(|point| point.status != TouchStatus::Release);

        // The Core2 digitizer is 320x280: its bottom 40 rows are the three
        // capacitive bezel buttons, outside the 320x240 LCD. Do not turn those
        // samples into clamped touches on the bottom edge of the Slint window.
        Ok(point
            .filter(|point| point.y < DISPLAY_HEIGHT)
            .map(|point| TouchPoint::new(point.x, point.y)))
    }
}
