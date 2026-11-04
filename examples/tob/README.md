# Total-order broadcast поверх HotStuff-2

Модуль total-order broadcast из *Introduction to Reliable and Secure Distributed
Programming* (Cachin, Guerraoui, Rodrigues), собранный из ноды консенсуса.
Кластер из четырёх нод крутится в одном процессе.

| Интерфейс модуля          | Здесь                                   |
|---------------------------|-----------------------------------------|
| `⟨tob, Broadcast \| m⟩`   | `Tob::broadcast` → `Client::propose`    |
| `⟨tob, Deliver \| p, m⟩`  | `Output::on_committed` → `Tob::deliver` |

## Раскладка

```
src/lib.rs    Message, Batch (команда), BatchMempool, Deliveries (Output), Tob, spawn_cluster
src/main.rs   каждая нода рассылает 5 сообщений, все ноды печатают порядок доставки
```

Команда консенсуса — пачка сообщений `Batch`. Пустая пачка законна, поэтому
лидеру всегда есть что предложить, даже если мемпул пуст.

## Запуск

```bash
cargo run -p tob
```

```
node 0: 2/m0 2/m1 ... 1/m4
...
all 4 nodes delivered 20 messages in the same order
```

## Ограничения

- Подписи фиктивные (`TestScheme`), сеть — `MemoryNetwork` в одном процессе.
- Сообщение попадает только в мемпул своей ноды и предлагается, когда она
  становится лидером.
- Мемпул отдаёт сообщения при предложении и не возвращает их обратно: если блок
  лидера не будет закоммичен (например, следующий лидер упал), его сообщения
  потеряются, и validity не выполняется.
- Пока мемпулы пусты, лидеры без паузы предлагают пустые пачки.
