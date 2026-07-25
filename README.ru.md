# slint-adapter

[English](README.md) | **Русский**

`slint-adapter` — небольшой `#![no_std]`-адаптер платформы Slint для
микроконтроллеров. Он соединяет:

- настроенный, но ещё не инициализированный `mipidsi::Builder`;
- реализацию источника касаний `TouchInput`;
- монотонные часы и ожидание через `McuRuntime`;
- программный renderer Slint.

После инициализации адаптер возвращает сгенерированный Slint-компонент без
обёрток. С ним можно работать как с обычным приложением Slint: менять свойства,
подписываться на callbacks, вызывать `show()` или `run()`.

Крейт рассчитан на Slint 1.17, `mipidsi` 0.10 и `embedded-hal` 1.0.

## Что делает адаптер

- Принимает готовый `mipidsi::Builder` вместе с выбранной моделью дисплея,
  интерфейсом, ориентацией и reset-пином.
- Вызывает `mipidsi::Builder::init`.
- Устанавливает глобальную реализацию `slint::platform::Platform`.
- Рисует интерфейс построчно в формате RGB565.
- Хранит только одну строку пикселей: приблизительно `ширина × 2` байта вместо
  framebuffer на весь экран.
- Преобразует последовательность состояний тача в Slint-события указателя.
- Поддерживает блокирующий event loop и пошаговый event loop для Embassy или
  другого кооперативного executor.

Адаптер не настраивает SPI/I²C, частоты шин, питание, подсветку, аппаратную
калибровку тача и глобальный allocator. Эти части зависят от конкретной платы и
остаются в BSP приложения.

## Зависимость

```toml
[dependencies]
slint-adapter = "0.1.0"

slint = { version = "1.17.1", default-features = false, features = [
    "compat-1-2",
    "unsafe-single-threaded",
    "libm",
    "renderer-software",
] }

[build-dependencies]
slint-build = "1.17.1"
```

На bare metal Slint нужен глобальный allocator. Его необходимо создать до
конструирования Slint-компонента.

Ресурсы интерфейса следует подготовить для software renderer:

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

## Базовое использование

Сначала приложение настраивает питание, GPIO, SPI, I²C и контроллеры платы.
После этого сконфигурированный `mipidsi::Builder`, тач и runtime передаются в
`AdapterBuilder`.

```rust,ignore
#![no_std]
#![no_main]

extern crate alloc;

use core::time::Duration;
use slint::ComponentHandle;
use slint_adapter::{
    AdapterBuilder, McuRuntime, TouchInput, TouchPoint,
};

slint::include_modules!();

struct BoardTouch {
    // I²C-драйвер тача, состояние калибровки и т. п.
}

impl TouchInput for BoardTouch {
    type Error = TouchError;

    fn read_touch(&mut self) -> Result<Option<TouchPoint>, Self::Error> {
        let sample = self.driver.scan()?;

        Ok(sample.map(|sample| {
            // Здесь выполняются калибровка и преобразование ориентации.
            TouchPoint::new(sample.x, sample.y)
        }))
    }
}

struct BoardRuntime {
    // Монотонный таймер платы.
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
    // 1. Инициализировать allocator, питание, GPIO, SPI и I²C.
    // 2. Включить питание и подсветку дисплея.
    // 3. При необходимости аппаратно сбросить дисплей и touch-контроллер.

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

    // Возвращён настоящий сгенерированный компонент Slint.
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

## Использование с Embassy

Нельзя просто запустить блокирующий `app.run()` внутри Embassy-задачи: такой
вызов не возвращает управление executor. Для async-приложения используйте
`build_with_event_loop`.

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

Один вызов `McuEventLoop::step`:

1. обновляет таймеры и анимации Slint;
2. читает одно текущее состояние тача;
3. отправляет pointer event в окно;
4. рисует накопившиеся изменения;
5. возвращает максимальное время до следующего вызова.

Готовая реализация для ESP32 находится в
[`examples/m5stack-core2`](https://github.com/antonsterkhov/slint-adapter/tree/main/examples/m5stack-core2).

## Дисплей без тача

Если в устройстве нет touch-контроллера или ввод полностью реализован другим
способом, используйте `AdapterBuilderWithoutTouch`:

```rust,ignore
use slint_adapter::AdapterBuilderWithoutTouch;

let app = AdapterBuilderWithoutTouch::new(display_builder, runtime)
    .build(&mut delay, AppWindow::new)?;

app.run()?;
```

Этот же builder поддерживает кооперативный event loop:

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

`AdapterBuilderWithoutTouch` устанавливает [`NoTouch`] внутри автоматически.
Приложению не требуется создавать фиктивную реализацию `TouchInput`. Таймеры и
анимации Slint, изменение свойств, вызов callbacks из прикладного кода и
отрисовка продолжают работать как обычно; платформа просто не генерирует
pointer events.

# Подробно о таче

## Контракт `TouchInput`

`TouchInput` — синхронный polling-интерфейс:

```rust,ignore
pub trait TouchInput {
    type Error;

    fn read_touch(&mut self)
        -> Result<Option<TouchPoint>, Self::Error>;
}
```

Метод должен возвращать **текущее состояние основного контакта**, а не очередь
разовых событий:

- `Ok(Some(point))` — палец сейчас касается экрана;
- `Ok(None)` — активного касания сейчас нет;
- `Err(error)` — контроллер или шина не смогли выполнить чтение.

Пока палец удерживается, реализация должна возвращать `Some(point)` при каждом
опросе. Нельзя вернуть координату только в момент первого IRQ, а затем постоянно
возвращать `None`: адаптер воспримет это как немедленное отпускание.

## Как состояния превращаются в события Slint

Адаптер запоминает предыдущее состояние и формирует события автоматически:

| Предыдущее состояние | Новое состояние | Событие Slint |
|---|---|---|
| `None` | `None` | нет события |
| `None` | `Some(point)` | `PointerPressed` |
| `Some(old)` | `Some(new)`, координаты изменились | `PointerMoved` |
| `Some(point)` | та же точка | нет события |
| `Some(old)` | `None` | `PointerReleased`, затем `PointerExited` |

В результате стандартные компоненты Slint (`Button`, `TouchArea`, `Slider` и
другие) работают без специального кода в `.slint`-файле:

```slint
import { Button } from "std-widgets.slint";

export component AppWindow inherits Window {
    in-out property <int> counter: 0;

    Button {
        text: "Нажать";
        clicked => {
            root.counter += 1;
        }
    }
}
```

Текущая версия адаптера использует только один основной контакт и кнопку
`PointerEventButton::Left`. Если touch-контроллер поддерживает multitouch,
реализация `TouchInput` должна выбрать один контакт, обычно первый активный.

## Минимальная реализация

Если драйвер уже возвращает откалиброванные координаты экрана, адаптер выглядит
так:

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

Для устройства без тача предпочтительно использовать
`AdapterBuilderWithoutTouch`. Низкоуровневый `NoTouch` остаётся доступен для
обобщённого кода, который создаёт `AdapterBuilder` напрямую.

## Пример для FT6336U

`ft6336u-driver` возвращает до двух точек. Для Slint выбирается первая точка,
которая не находится в состоянии `Release`:

```rust,ignore
use embedded_hal::i2c::I2c;
use ft6336u_driver::{
    Error, FT6336U, GestureMode, TouchStatus,
};
use slint_adapter::{TouchInput, TouchPoint};

struct Ft6336Touch<I2C> {
    controller: FT6336U<I2C>,
}

impl<I2C> Ft6336Touch<I2C>
where
    I2C: I2c,
{
    fn new(i2c: I2C) -> Result<Self, Error<I2C::Error>> {
        let mut controller = FT6336U::new(i2c);

        // Адаптер регулярно опрашивает состояние, поэтому polling mode
        // является самым простым и надёжным вариантом.
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

Если PMIC, тач, RTC и другие устройства используют одну I²C-шину, каждому
драйверу можно передать виртуальное устройство из `embedded-hal-bus`:

```rust,ignore
use core::cell::RefCell;
use embedded_hal_bus::i2c::RefCellDevice;

let bus = RefCell::new(i2c);

let touch_i2c = RefCellDevice::new(&bus);
let pmic_i2c = RefCellDevice::new(&bus);

let touch = Ft6336Touch::new(touch_i2c)?;
let power = Axp192::new(pmic_i2c);
```

`RefCellDevice` подходит только для однопоточного доступа. Это соответствует
режиму Slint `unsafe-single-threaded`, если все обращения выполняются в одной
задаче и не происходят из interrupt handler.

## Координаты, поворот и зеркалирование

`TouchPoint` должен быть указан в **логической системе координат дисплея после
настройки `mipidsi::Builder`**:

- `(0, 0)` — левый верхний угол Slint-окна;
- `x` растёт вправо;
- `y` растёт вниз;
- максимальные значения обычно равны `width - 1` и `height - 1`.

Если LCD повёрнут через `mipidsi::Builder::orientation`, raw-координаты тача
не обязательно поворачиваются автоматически. Преобразование выполняет BSP или
реализация `TouchInput`.

Для raw-панели размером `W × H` типовые преобразования выглядят так:

| Ориентация | Логическая координата |
|---|---|
| 0° | `(x, y)` |
| 90° по часовой стрелке | `(H - 1 - y, x)` |
| 180° | `(W - 1 - x, H - 1 - y)` |
| 270° по часовой стрелке | `(y, W - 1 - x)` |

Конкретный контроллер может уже менять оси или инвертировать одну из них,
поэтому преобразование необходимо проверить по четырём углам реального экрана.

Адаптер ограничивает готовые координаты размерами дисплея. Это защищает от
единичных выбросов, но не заменяет калибровку.

## Калибровка raw-координат

Если контроллер возвращает диапазон `raw_min..raw_max`, его следует привести к
размеру Slint-окна:

```rust,ignore
fn scale_axis(raw: u16, raw_min: u16, raw_max: u16, screen_size: u16) -> u16 {
    let raw = raw.clamp(raw_min, raw_max);
    let input = u32::from(raw - raw_min);
    let input_range = u32::from(raw_max - raw_min).max(1);
    let output_range = u32::from(screen_size.saturating_sub(1));

    (input * output_range / input_range) as u16
}
```

Для более точной резистивной или нелинейной панели можно применить affine
transform перед созданием `TouchPoint`.

Некоторые панели имеют активную область больше LCD. Например, у M5Stack Core2
FT6336U выдаёт область `320×280`: нижние 40 строк соответствуют трём сенсорным
зонам под экраном. Если они не используются интерфейсом, такие точки лучше
отбрасывать:

```rust,ignore
if raw_y >= DISPLAY_HEIGHT {
    return Ok(None);
}
```

Не следует полагаться на автоматическое ограничение координат: иначе нажатие
под экраном превратится в ложное нажатие на его нижней границе.

## Polling и interrupt

По умолчанию адаптер опрашивает тач не реже одного раза за заданный
`poll_interval`. Значение по умолчанию — `16_667` микросекунд, приблизительно
60 Гц:

```rust,ignore
let builder = AdapterBuilder::new(display, touch, runtime)
    .poll_interval(core::time::Duration::from_millis(10));
```

Меньший интервал уменьшает задержку ввода, но увеличивает количество операций
I²C и время работы CPU.

IRQ можно использовать для пробуждения MCU или для выставления флага
«контроллер нужно прочитать». При этом:

- нельзя вызывать Slint API непосредственно из interrupt handler;
- I²C-чтение лучше выполнять в обычной задаче;
- после первого IRQ необходимо продолжать возвращать `Some(point)`, пока палец
  удерживается;
- отпускание тоже должно превратиться в один вызов с `Ok(None)`.

Если IRQ контроллера является коротким импульсом, простая проверка текущего
уровня GPIO внутри `read_touch()` может пропускать события. В таком случае IRQ
должен защёлкивать флаг, либо контроллер следует перевести в polling mode.

## Ошибки чтения

Ошибка `TouchInput::Error` преобразуется в `slint::PlatformError`.

- При блокирующем `app.run()` event loop завершится с ошибкой.
- При ручном `event_loop.step()` ошибка вернётся из текущего шага.

Для надёжного устройства полезно отделить временную ошибку I²C от постоянной.
Например, BSP может повторить чтение, восстановить шину или пропустить один
sample. Важно не превращать каждую временную ошибку автоматически в
`Ok(None)`: во время удержания это создаст ложные `PointerReleased` и следующий
`PointerPressed`.

## Проверка реализации тача

Перед подключением сложного UI полезно проверить:

1. нажатие в четырёх углах;
2. движение по горизонтали и вертикали;
3. удержание без ложных отпусканий;
4. корректный release;
5. отсутствие событий за пределами LCD;
6. работу после пробуждения и повторной инициализации I²C;
7. соответствие координат выбранному повороту `mipidsi`.

После этого стандартная кнопка Slint должна нажиматься, менять визуальное
состояние при удержании и получать `clicked` только после корректной
последовательности press/release.

## Контракт `McuRuntime`

`McuRuntime::now` должен возвращать монотонное время. Slint использует его для
таймеров и анимаций.

`McuRuntime::wait` получает минимальное значение из:

- времени до ближайшего Slint-таймера;
- настроенного `poll_interval`.

Метод может вернуться раньше, например после touch IRQ. В пошаговом Embassy
event loop ожидание выполняет сама async-задача, но runtime всё равно хранится
в установленной платформе и позволяет использовать обычный `app.run()`.

Все вызовы Slint должны оставаться в одном потоке/задаче и не должны
выполняться из interrupt handler.

## Дисплей и память

Построчный renderer использует `Rgb565Pixel` и вызывает
`DrawTarget::fill_contiguous` для изменившегося участка строки.

Форматы `Rgb565` и `Rgb666`, доступные в `mipidsi` 0.10, поддерживаются через
преобразование из `embedded_graphics_core::pixelcolor::Rgb565`.

Адаптер не использует DMA и не владеет настройкой SPI. Оптимизацию частоты SPI,
размера буфера display-interface и DMA следует выполнять в BSP конкретного MCU.

## Ссылки

- [Slint на микроконтроллерах](https://docs.slint.dev/latest/docs/rust/slint/docs/mcu/)
- [Slint `Platform`](https://docs.slint.dev/latest/docs/rust/slint/platform/trait.Platform)
- [Slint `LineBufferProvider`](https://docs.slint.dev/latest/docs/rust/slint/platform/software_renderer/trait.LineBufferProvider)
- [`mipidsi::Builder`](https://docs.rs/mipidsi/0.10.0/mipidsi/struct.Builder.html)
- [`mipidsi::Display`](https://docs.rs/mipidsi/0.10.0/mipidsi/struct.Display.html)
- [Аппаратный пример M5Stack Core2](https://github.com/antonsterkhov/slint-adapter/blob/main/examples/m5stack-core2/README.ru.md)
