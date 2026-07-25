use axp192::{Axp192, GpioMode12, GpioMode34};
use embedded_hal::{delay::DelayNs, i2c::I2c};

const LCD_LOGIC_MV: u16 = 3_300;
const BACKLIGHT_MIN_MV: u16 = 2_500;
const BACKLIGHT_MAX_MV: u16 = 3_275;
const DEFAULT_BACKLIGHT_PERCENT: u8 = 40;

/// Power rails and the shared LCD/touch reset line on the Core2's AXP192.
pub struct Core2Power<I2C> {
    axp: Axp192<I2C>,
    backlight_percent: u8,
}

impl<I2C> Core2Power<I2C>
where
    I2C: I2c,
{
    /// Initializes only the rails needed by the LCD and touch panel.
    pub fn initialize(i2c: I2C, delay: &mut impl DelayNs) -> Result<Self, I2C::Error> {
        let mut power = Self {
            axp: Axp192::new(i2c),
            backlight_percent: DEFAULT_BACKLIGHT_PERCENT,
        };

        // LDO2 supplies the LCD logic and the board's peripheral rail.
        power.axp.set_ldo2_voltage(LCD_LOGIC_MV)?;

        // LDO3 drives the vibration motor on Core2. Keep it disabled.
        power.axp.set_ldo3_on(false)?;

        // GPIO2 enables the speaker amplifier. Keep it quiet during bring-up.
        power.axp.set_gpio2_mode(GpioMode12::NmosOpenDrainOutput)?;
        power.axp.set_gpio2_output(false)?;

        power.set_screen_power(true)?;

        // AXP192 GPIO4 resets both ILI9342C and FT6336U.
        power.axp.set_gpio4_mode(GpioMode34::NmosOpenDrainOutput)?;
        power.axp.set_gpio4_output(false)?;
        delay.delay_ms(10);
        power.axp.set_gpio4_output(true)?;
        delay.delay_ms(120);

        Ok(power)
    }

    /// Sets the Core2 backlight from fully off (`0`) to maximum (`100`).
    pub fn set_backlight(&mut self, percent: u8) -> Result<(), I2C::Error> {
        let percent = percent.min(100);
        self.backlight_percent = percent;

        if percent == 0 {
            return self.axp.set_dcdc3_on(false);
        }

        let span = u32::from(BACKLIGHT_MAX_MV - BACKLIGHT_MIN_MV);
        let millivolts = u32::from(BACKLIGHT_MIN_MV) + span * u32::from(percent) / 100;
        self.axp.set_dcdc3_voltage(millivolts as u16)?;
        self.axp.set_dcdc3_on(true)
    }

    /// Enables or disables both the LCD logic rail and its backlight.
    pub fn set_screen_power(&mut self, enabled: bool) -> Result<(), I2C::Error> {
        if enabled {
            self.axp.set_ldo2_on(true)?;
            self.set_backlight(self.backlight_percent.max(1))
        } else {
            self.axp.set_dcdc3_on(false)?;
            self.axp.set_ldo2_on(false)
        }
    }
}
