# Model-based testing: Quint → Rust

Минимальный работающий пример связки формальной спецификации с реализацией.
Спека описывает набор независимых счётчиков, Quint генерирует из неё случайные
трассы, а [`quint-connect`](https://crates.io/crates/quint-connect) проигрывает
их против настоящего Rust-кода и сверяет состояния после каждого шага.

Счётчики выбраны намеренно примитивными: интересна тут не предметная область, а
сам механизм связки — на нём же строится MBT для консенсуса.

## Раскладка

```
spec/counter.qnt     спецификация: действия, инварианты, сценарии
src/lib.rs           реализация — про Quint ничего не знает
tests/mbt.rs         обвязка: Driver + State + генерация трасс
```

Разделение существенное: `src/lib.rs` — обычный код, который можно тестировать
руками. Вся связь со спекой живёт в `tests/`.

## Запуск

Только спека, без Rust:

```bash
cd spec
quint typecheck counter.qnt
quint test counter.qnt                                   # сценарии
quint run counter.qnt --invariant=keysStableInv \
    --max-steps=10 --max-samples=500                     # случайный обход
```

Вся связка целиком:

```bash
cargo test -p quint-counter
```

Требуется `quint` в `PATH` — крейт запускает его сам.

## Как связаны две стороны

Quint вызывается с флагом `--mbt`, и тогда каждое состояние трассы несёт
дополнительные поля:

```json
{
  "mbt::actionTaken": "increment",
  "mbt::nondetPicks": { "pid": {"tag":"Some","value":"p2"},
                        "delta": {"tag":"Some","value":2} },
  "counters": { "#map": [["p1", 0], ["p2", 2], ["p3", 0]] }
}
```

Отсюда три вещи:

| Из трассы | Куда попадает в Rust |
|---|---|
| `mbt::actionTaken` | имя ветки в `switch!` |
| `mbt::nondetPicks` | параметры ветки — по именам `nondet`-связываний |
| состояние | сравнивается с `State::from_driver` |

Имена должны совпадать буквально: `nondet pid = oneOf(...)` в спеке →
`increment(pid: String, ...)` в `switch!`, поле `counters` в спеке → поле
`counters` в `SpecState`.

Важная деталь: в `actionTaken` попадает **внутреннее** действие (`increment`),
а не обёртка `step`.

## Проверка, что оно вообще работает

Внесите ошибку в реализацию:

```rust
// src/lib.rs
pub fn decrement(&mut self, pid: &str, delta: i64) -> Result<(), UnknownProcess> {
    self.apply(pid, -delta + 1)   // off-by-one
}
```

```
$ cargo test -p quint-counter --test mbt
   Generating 20 traces using `0xebfd164f` as random seed ...
   Replaying traces ...
   Specification and implementation states diverge
   [FAIL] random_traces
   Reproduce this error with `QUINT_SEED=0xebfd164f`
```

## Отладка расхождений

По умолчанию печатается только строчка «states diverge» — что именно разошлось,
не видно. Диф включается переменной:

```bash
QUINT_VERBOSE=1 cargo test -p quint-counter --test mbt -- --nocapture
```

```
   Specification and implementation states diverge
   --- specification
   +++ implementation
    SpecState {
        counters: {
            "p1": 0,
            "p2": 0,
   -        "p3": -3,
   +        "p3": -2,
        },
    }
```

`QUINT_VERBOSE=3` добавляет пошаговую распечатку: имя действия, `nondet`-пики и
состояние на каждом шаге.

Повторить конкретный прогон:

```bash
QUINT_SEED=0xebfd164f cargo test -p quint-counter --test mbt
```

Обе переменные читаются через `option_env!`, то есть попадают в код при сборке.
Cargo это отслеживает и пересобирает `quint-connect` сам при их изменении —
чистить вручную не нужно, но первый запуск после смены будет дольше.

## Две шероховатости, на которые мы напоролись

**`anyhow` нужен в зависимостях явно.** Макрос `switch!` раскрывается в код,
который ссылается на `anyhow::` напрямую, без ре-экспорта. Без этой зависимости
сборка падает с `use of unresolved module or unlinked crate anyhow`.

**`#[quint_test]` не работает с Quint 0.32.0.** Макрос генерирует трассы командой
`quint test`, а у неё нет флага `--mbt` (в отличие от `quint run`). В результате
`mbt::actionTaken` в трассу не попадает и драйвер падает с
`Missing mbt::actionTaken variable in the trace`. Поэтому здесь используется
только `#[quint_run]`; сценарные прогоны из спеки проверяются напрямую через
`quint test`.

Версии, на которых это проверено: quint 0.32.0, quint-connect 0.1.2.
