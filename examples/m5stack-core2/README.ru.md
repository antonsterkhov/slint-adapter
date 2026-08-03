# M5Stack Core2: Slint + esp-hal + Embassy

[English](README.md) | **Русский**

Аппаратный пример для оригинального M5Stack Core2 на ESP32 и AXP192. Проект
создан `esp-generate 1.3.0` и использует bare-metal `no_std` стек:

- `esp-hal 1.1` для GPIO, I²C, SPI и таймеров;
- `esp-rtos 0.3` для интеграции Embassy;
- `mipidsi 0.10` и `ILI9342CRgb565` для LCD;
- `ft6336u-driver 1.0` для тач-контроллера;
- `axp192 0.2` для питания LCD, подсветки и общей линии reset;
- локальный `slint-mipidsi-adapter` и Slint software renderer.

## Подключённая периферия

| Узел | Подключение Core2 |
|---|---|
| ILI9342C | SCLK GPIO18, MOSI GPIO23, CS GPIO5, D/C GPIO15 |
| FT6336U | I²C 0x38, SDA GPIO21, SCL GPIO22, INT GPIO39 |
| AXP192 | I²C 0x34, SDA GPIO21, SCL GPIO22 |
| LCD/touch reset | AXP192 GPIO4 |
| LCD/peripheral power | AXP192 LDO2, 3.3 V |
| Backlight | AXP192 DCDC3 |

GPIO38/MISO дисплея не используется: текущий renderer только записывает пиксели.
Тач и PMIC получают два `RefCellDevice` поверх одной физической I²C-шины.

AXP192 на старте включает LDO2, выставляет безопасную начальную яркость через
DCDC3, отключает вибромотор и усилитель динамика, затем аппаратно сбрасывает LCD
и touch через GPIO4. В `power.rs` также есть методы изменения яркости и
включения/отключения экрана.

## Сборка и прошивка

Нужны установленный Espressif Rust toolchain и `espflash`. Из каталога примера:

```text
cargo check
cargo run --release
```

Локальный `.cargo/config.toml` уже выбирает `xtensa-esp32-none-elf` и запускает:

```text
espflash flash --monitor --chip esp32
```

Если подключено несколько последовательных портов, `espflash` предложит выбрать
нужный.

Для прямой прошивки на известный порт:

```text
espflash flash --chip esp32 --port COM11 \
    target/xtensa-esp32-none-elf/release/m5stack-core2
```

## Архитектура выполнения

`main` настраивает питание и шины, запускает Embassy и передаёт железо в
`ui_task`. UI task:

1. инициализирует ILI9342C из настроенного `mipidsi::Builder`;
2. создаёт обычный сгенерированный `AppWindow`;
3. показывает окно;
4. делает один шаг Slint event loop;
5. асинхронно спит до ближайшего Slint-таймера или следующего touch poll.

Поэтому рендеринг не блокирует Embassy executor навсегда, а основной task
остаётся доступен для будущей бизнес-логики и другой периферии.

Дигитайзер Core2 имеет область 320x280, включая три сенсорные зоны под LCD.
Этот пример передаёт Slint только координаты внутри видимой области 320x240.

## Работа тача

FT6336U настроен в polling mode. На каждом шаге event loop метод
`Core2Touch::read_touch` опрашивает контроллер и выбирает первый активный
контакт.

Область дигитайзера Core2 равна `320×280` и включает три сенсорные зоны под
LCD `320×240`. Пример передаёт в Slint только точки с `y < 240`. Касания ниже
видимой области отбрасываются, а не прижимаются к нижней границе экрана.

`slint-mipidsi-adapter` превращает последовательность состояний в события:

- первый контакт → `PointerPressed`;
- изменение координат → `PointerMoved`;
- отсутствие контакта после удержания → `PointerReleased`.

В демонстрации есть два экрана. На главном экране:

- `Add click` вызывает Slint callback, который обрабатывается в Rust и
  увеличивает счётчик;
- `Open details` вызывает другой Rust callback и открывает экран деталей.

Экран деталей отображает тот же счётчик, а кнопка `Back to home` возвращает на
главный экран через третий Rust callback. Это показывает, что возвращённое
`slint-mipidsi-adapter` приложение предоставляет обычный сгенерированный API Slint:
callbacks `on_*` и свойства через `get_*`/`set_*`.

## Важно о ревизии платы

Этот вариант предназначен для Core2 с **AXP192**. У Core2 v1.1 другая схема
питания на AXP2101; перед прошивкой такой ревизии нужен отдельный power backend.

## Связанная документация

- [Основная документация `slint-mipidsi-adapter`](../../README.ru.md)
- [Основная документация на английском](../../README.md)
- [Документация M5Stack Core2](https://docs.m5stack.com/en/core/core2)
- [Rust on ESP: esp-generate](https://docs.espressif.com/projects/rust/book/getting-started/tooling/esp-generate.html)
- [Rust on ESP: async и Embassy](https://docs.espressif.com/projects/rust/book/application-development/async.html)
- [Slint на микроконтроллерах](https://docs.slint.dev/latest/docs/rust/slint/docs/mcu/)
