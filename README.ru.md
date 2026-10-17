<div align="center">

<img src="docs/assets/hero.ru.svg" alt="AgentGit: система контроля версий для мира, где код пишут и агенты. Четыре независимых вердикта на изменение: Mergeable, Validated, Approved, Integrated." width="100%">

<br>

[![CI](https://img.shields.io/github/actions/workflow/status/Paffin/AgentGit/ci.yml?branch=main&style=flat-square&label=CI)](https://github.com/Paffin/AgentGit/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square)](LICENSE)
[![Rust 1.89+](https://img.shields.io/badge/rust-1.89%2B-orange?style=flat-square)](Cargo.toml)
[![Protocol: AGWP v1](https://img.shields.io/badge/protocol-AGWP%20v1-5b6cff?style=flat-square)](docs/protocol/agwp-v1.md)
[![Status: alpha](https://img.shields.io/badge/status-alpha-yellow?style=flat-square)](#статус-и-честные-ограничения)

[English](README.md) · **Русский** · [Документация](docs/README.md) · [Roadmap](ROADMAP.md) · [Архитектура](docs/architecture/overview.md)

[**Как это выглядит**](#как-это-выглядит) ·
[**Установка**](#установка) ·
[**Быстрый старт**](#быстрый-старт) ·
[**Сервер**](#запуск-как-сервера) ·
[**Как это работает**](#как-это-работает) ·
[**Честные ограничения**](#статус-и-честные-ограничения)

</div>

<br>

> Документация в `docs/` написана на английском; ADR-0001 (исходное архитектурное решение) — на русском:
> [`docs/adr/0001-final-architecture.md`](docs/adr/0001-final-architecture.md).

**AgentGit — agent-native SCM** с собственным детерминированным VCS-ядром: безопасная при сбоях конкурентная
интеграция, валидация, привязанная к *точному дереву*, которое проверяли, политики автономности агентов и
аудит, в котором видна любая подмена. Ядро, которое хранит, сливает и восстанавливает, никогда не требует
языковой модели; всё «умное» — надстройка над ним, и она может только *предлагать*.

## Зачем

Git создавался для одного человека за одной клавиатурой. Когда десять агентов и три человека одновременно
меняют один код, эти вопросы перестают быть академическими:

| Когда агенты и люди делят один код… | …ответ AgentGit |
|---|---|
| 🪪 **Кто именно** сделал изменение: запуск агента или человек? | Идентичность на каждого участника *и на каждый запуск* и подписанный **provenance** рядом с неизменяемой историей. |
| 🧪 **Что на самом деле проверяли?** *Это* дерево или то, что было до чужого merge? | **Аттестации** называют точное дерево, версию политики и окружение. Поток сдвинулся — проверка больше не засчитывается. |
| 🤖 **Может ли агент сам влить код?** В какие файлы? Сколько? | **Политика** потока с *конвертом автономности*. Агенты не могут интегрировать, пока политика этого не разрешит. |
| 💥 Два агента гонятся: **может ли обновление потеряться?** Процесс упал посреди merge: **в каком состоянии репозиторий?** | Голова двигается через **compare-and-swap**; сбои внедряются на каждом шаге протокола. Результат — ровно один исход. |
| 🔍 Можно ли **потом доказать**, что произошло и кто это разрешил? | Append-only reflog, **аудит с хеш-цепочкой** и `ag verify` для всего репозитория. |

## Как это выглядит

Человек, два агента и CI-сервис работают с одним потоком. (Вывод сокращён из
[`examples/quickstart.sh`](examples/quickstart.sh) — CI запускает этот сценарий при каждом push.)

```text
$ ag integrate prop_6eed…                  # as agent:refactor-bot — агент пытается сам влить код
Denied by policy; the stream was not changed:
  - actor not allowed to integrate: agents may not integrate unless agent_autonomy.auto_integrate is on …

$ ag integrate prop_6eed…                  # as user:alice
Blocked (policy v1); the stream was not changed:
  - check 'tests' has no trusted passing attestation for this exact tree
  - approvals (default): 0 of 1 required
  - agent-authored change has no signed provenance
  - exclusive lease on 'src/**' held by agent:refactor-bot#run_8f78… (stale_or_missing_token)
candidate tree: 315f74d45183c9e7…

$ ag validate run prop_6eed… --check tests --key ci -- cargo test        # as service:ci
check 'tests' PASSED on tree 315f74d45183 (policy v1); attestation att_0f1a… signed by service:ci
$ ag proposal review prop_6eed… --approve                                # as user:bob
$ ag integrate prop_6eed… --fencing-token 1                              # as user:alice
Integrated (merge): stream is now at 793a03b6da69 (version 3, was 70e31c72a4d0), 1 file(s) changed.

$ ag preview prop_2b19…                                                  # параллельная правка той же строки
strategy: Merge   (algorithm ag-merge/2+ag-diff/1)
conflict: src/main.rs (content conflict (1 region(s)))                   # код выхода 4 — поток не тронут
```

> [!TIP]
> Каждый отказ говорит, *чего не хватает*, а коды выхода подсказывают скриптам и агентам, что делать дальше:
> `3` — заблокировано (предоставьте недостающее и повторите), `4` — отказ.

## Что внутри

<table>
<tr>
<td width="33%" valign="top">

#### 🛡️ Безопасность при сбоях по построению

Объекты проверяются *до* единственной транзакции публикации; голова потока двигается через compare-and-swap
с обязательным пересчётом. Fault injection на каждом шаге протокола.

</td>
<td width="33%" valign="top">

#### 🔀 Воспроизводимые merge

Чистая версионированная функция от base, ours и theirs. Типизированные конфликты, ничего не угадывается.
Перемещения учитываются: файл, переименованный на одной стороне и изменённый на другой, просто сливается.

</td>
<td width="33%" valign="top">

#### ✅ Валидация привязана к точному дереву

Подписанные аттестации называют *дерево*, версию политики и окружение. Поток сдвинулся или кандидат
изменился — проверьте заново.

</td>
</tr>
<tr>
<td valign="top">

#### 🤖 Агенты в рамках политики

Доверенные эмитенты, обязательные проверки, одобрения, привязанные к ревьюенной голове, защищённые пути и
*конверт автономности*: какие пути и сколько файлов агент вправе влить сам.

</td>
<td valign="top">

#### 🤝 Координация агентов

Идентичность на каждый запуск, задачи с зависимостями, мягкие и эксклюзивные **lease с fencing-токенами**,
**предсказание** конфликтов и упорядоченная **очередь** интеграции.

</td>
<td valign="top">

#### 🧱 Изолированное исполнение

`ag-worker` забирает задания на валидацию, запускает команду из политики в песочнице `ag-sandbox`
(rootless-namespaces или контейнер) и подписывает результат для точного дерева.

</td>
</tr>
<tr>
<td valign="top">

#### 📜 История, которой можно верить

Неизменяемые content-addressed changesets с проверкой хеша при каждом чтении; append-only reflog; аудит с
хеш-цепочкой. Построчный `ag blame` с переходом через переименования.

</td>
<td valign="top">

#### 🧰 Безопасная эксплуатация

GC, который не удаляет достижимое и не теряет только что упомянутый объект; согласованные проверенные бэкапы;
одноразовые, пересобираемые индексы.

</td>
<td valign="top">

#### 🔁 Git как адаптер

`ag git import` / `export`: деревья проходят round-trip с идентичными Git SHA. Плюс `clone` / `fetch` / `pull` /
`push` и полностью локальная офлайн-работа.

</td>
</tr>
<tr>
<td valign="top">

#### 🌐 Сетевой сервер

Один протокол (AGWP v1) по gRPC и HTTP/JSON на **одном порту**: OIDC или scoped-credentials с коротким сроком
жизни, RBAC на репозиторий, квоты, живые события (SSE). Политика действует и против клиентов, которых вы не
контролируете.

</td>
<td valign="top">

#### 🗄️ PostgreSQL, S3, наблюдаемость

Метаданные PostgreSQL на репозиторий и S3-совместимые объекты; метрики Prometheus, трейсы OpenTelemetry,
готовые алерты и дашборды.

</td>
<td valign="top">

#### 🧩 Клиенты

[Web UI](docs/guides/web-ui.md) вокруг живой работы агентов, SDK на [TypeScript и Python](docs/guides/sdks.md) и
[платформа агентов](docs/guides/orchestrator.md) на Go, которая может предлагать, но не интегрировать.

</td>
</tr>
</table>

### Web UI

Живая доска запусков, lease, задач и очереди слияния, по четыре независимых вердикта на каждое предложение.
(Настоящие скриншоты UI, работающего против настоящего сервера; их снимает сквозной набор тестов.)

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/ui-board-dark.png">
    <img src="docs/assets/ui-board.png" alt="Живая доска: запуски, lease, задачи, очередь слияния и четыре независимых вердикта по каждому предложению" width="100%">
  </picture>
</p>

<table>
<tr>
<td width="34%" align="center" valign="top">
<img src="docs/assets/ui-proposal.png" alt="Предложение: четыре вердикта, что мешает интеграции, и diff" width="100%">
<br><sub><b>Предложение</b>: четыре вердикта, что мешает, diff</sub>
</td>
<td width="33%" align="center" valign="top">
<img src="docs/assets/ui-admin.png" alt="Администрирование: версионируемая политика и diff между версиями" width="100%">
<br><sub><b>Политика</b>: версии, только добавление, diff</sub>
</td>
<td width="33%" align="center" valign="top">
<img src="docs/assets/ui-blame.png" alt="Построчный blame с переходом через переименования" width="100%">
<br><sub><b>Blame</b>: построчный, через переименования</sub>
</td>
</tr>
</table>

## Установка

Нужен Rust **1.89+** и C-компилятор (SQLite собирается внутри). `git` и `python3` — по желанию.

```bash
git clone https://github.com/Paffin/AgentGit && cd AgentGit
cargo install --path crates/ag-cli          # установит бинарник `ag`
```

## Быстрый старт

```bash
mkdir demo && cd demo
ag init --namespace me/demo                 # создаёт .ag/ и поток "main"
echo "# demo" > README.md
ag snapshot -m "first"                      # записать каталог (пока невидимо для других)
ag propose -t "Initial import"              # проверить + опубликовать + открыть proposal
ag integrate <id-предложения>               # атомарно сдвинуть "main"; достаточно уникального префикса id
ag log
```

Дальше: [политика](docs/guides/policy.md), [агенты](docs/guides/agents.md) или вся история целиком:
`./examples/quickstart.sh`. Подробно: [Getting started](docs/getting-started.md).

## Запуск как сервера

```bash
cargo install --path crates/ag-server
export AG_BOOTSTRAP_TOKEN=$(openssl rand -hex 32)          # учётные данные первого администратора
ag-server admin --data-dir ./ag-data create-repo acme/web
ag-server admin --data-dir ./ag-data grant --subject user:alice --repo 'acme/*' --role admin
export AG_TOKEN=$(ag-server admin --data-dir ./ag-data mint --subject user:alice --ttl 86400 | tail -n 1)
ag-server serve --data-dir ./ag-data &                      # 127.0.0.1:8080, gRPC + HTTP/JSON
ag clone http://127.0.0.1:8080/acme/web web && cd web      # дальше: правки, `ag snapshot`, `ag push`
```

| Хочу… | Читать |
|---|---|
| 🚢 **Развернуть** | [Развёртывание](docs/operations/deployment.md): Docker, Compose (с PostgreSQL и MinIO), Kubernetes, TLS, секреты · [Наблюдаемость](docs/operations/observability.md): метрики, алерты, дашборды, трейсы |
| 🔌 **Пользоваться** | [Работа с сервером из CLI](docs/guides/remote.md) · [справочник AGWP](docs/protocol/agwp-v1.md) · [`openapi.json`](docs/protocol/openapi.json) |

Что из этого проверено в реальном развёртывании, а что нет, сказано на странице про развёртывание (на английском).

## Как это работает

```mermaid
flowchart LR
    W["workspace<br/>snapshots"] -->|propose| P["публикация<br/>проверка каждого объекта,<br/>регистрация ОДНОЙ транзакцией"]
    P --> Q["proposal<br/>ревью привязаны к голове"]
    Q -->|integrate| C["кандидат<br/>fast-forward или<br/>детерминированный merge"]
    C --> G{"политика — в той же<br/>транзакции, что и коммит"}
    G -->|разрешено| S[("голова потока<br/>compare-and-swap<br/>+ reflog + аудит")]
    G -->|заблокировано / отказ| Q
    C -.->|конфликт| W

    classDef step fill:#e8eeff,stroke:#5b6cff,color:#1b2150
    classDef gate fill:#fff4d6,stroke:#c99a1e,color:#4a3508
    classDef done fill:#dcf5e8,stroke:#1f9d67,color:#0f3d2e
    class W,P,Q,C step
    class G gate
    class S done
```

### Четыре вердикта — четыре вида доказательств

Изменение проходит четыре **независимые** проверки. Ни одна не подразумевает другую, и каждая меняется сама по
себе, в реальном времени:

| Вердикт | Вопрос | Кто решает | Меняется, когда |
|---|---|---|---|
| 🔀 **Mergeable** | Есть ли одно однозначное результирующее дерево? | движок merge (`ag preview`) | правка на любой стороне не оставляет единого результата |
| 🧪 **Validated** | Прошли ли *доверенные* проверки на *этом дереве*? | подписанные аттестации | меняется дерево-кандидат или политика |
| 👍 **Approved** | Одобрили ли *эту голову* достаточно допустимых ревьюеров? | ревью + политика | автор делает новый push (одобрения привязаны к голове) или меняется планка в политике |
| 🏁 **Integrated** | Поток обновлён атомарно? | одна CAS-транзакция | никогда: это факт истории |

<p align="center">
  <img src="docs/assets/ui-proposal-verdicts.png" alt="Одно предложение в Web UI: Mergeable и Validated пройдены, Approved — 0 из 1, Integrated — ещё нет; ниже словами сказано, что мешает" width="100%">
  <br><sub>Те же четыре вердикта в Web UI и, словами, что стоит на пути.</sub>
</p>

Одобрение не отменяет проваленную проверку, смена политики переключает Approved или Validated, не трогая
Mergeable, а интегратор, пытающийся влить заблокированное изменение, получает причины словами и неподвижный
поток. Всё это проверяется сквозными тестами Web UI.

### Вся система

<p align="center">
  <img src="docs/assets/architecture.svg" alt="Архитектура: клиенты говорят с ag-server по AGWP v1; сервер запускает движок ag-integrate над детерминированным ядром, метаданные в SQLite или PostgreSQL, объекты в файлах или S3; отдельная плоскость исполнения (ag-worker и ag-sandbox) запускает недоверенные проверки." width="100%">
  <br><sub>Подписи на схеме — на английском: это имена из кода и протокола.</sub>
</p>

Три вещи на этой схеме — **границы доверия**, а не просто блоки: ядро принимает любое решение без модели, часов и
случайности; движок работает внутри сервера, поэтому политика действует там, где подключаются недоверенные
клиенты; а недоверенный код выполняется только в отдельной плоскости исполнения, которая может *сообщить*
результат, но не решает, засчитают ли его.

Подробнее: [Concepts](docs/concepts.md) · [Architecture](docs/architecture/overview.md) ·
[Publication & integration](docs/architecture/publication-and-integration.md) ·
[Merge semantics](docs/architecture/merge-semantics.md).

## Гарантии — и тесты, которые их доказывают

Утверждение стоит ровно столько, сколько его доказательство. У каждого есть тест в этом репозитории.

| Гарантия | Доказательство |
|---|---|
| Сбой на **любом** шаге публикации или интеграции восстанавливается ровно в одно обновление | `crash_at_every_step_recovers_to_exactly_one_integration` — сбои на 7 точках протокола |
| Нет потерянных обновлений при параллельных интеграторах; reflog — непрерывная цепочка | `concurrent_integrations_never_lose_an_update`, `concurrent_cas_from_many_connections_never_loses_an_update` |
| Потерянный ответ не приводит к повторному применению | `idempotent_replay_and_unknown_outcome_recovery` |
| Проверка старого кандидата не переиспользуется после сдвига потока | `attestation_cannot_be_reused_after_the_stream_moves` |
| «Протухший» держатель lock не может влить код в область, которой владеет другой | `fencing_tokens_are_strictly_increasing_and_reject_stale_holders` |
| Бинарные форматы совпадают с **независимой реализацией на Python** | `tests/conformance/` (blob ID, changeset, деревья, аттестации, цепочка аудита) |
| История Git проходит round-trip с идентичными SHA деревьев | `history_survives_a_round_trip_with_identical_git_trees` |
| GC не удаляет достижимое и не теряет объект, на который только что сослались | `gc_fails_closed_when_a_reachable_object_is_damaged`, `rewriting_an_old_unreachable_object_saves_it_from_a_concurrent_gc` |
| Checkout не выходит за свой каталог и не подсаживает `.git` | `checkout_refuses_to_write_through_symlinked_directories`, `hostile_git_content_cannot_escape_or_plant_dot_git` |
| Подмена аудита или объектов обнаруживается | `audit_chain_detects_tampering`, `fsck_reports_a_healthy_repository_and_finds_each_kind_of_damage` |

Всё вместе: `cargo test --workspace` (около 550 тестов, несколько минут), плюс наборы SDK, Web UI и
оркестратора, которые идут против настоящего `ag-server`. Модель угроз, меры защиты и известные ограничения:
[Security](docs/architecture/security.md).

## Документация

| Хочу… | Начать отсюда |
|---|---|
| 🧪 **Попробовать** | [Getting started](docs/getting-started.md) · [Concepts](docs/concepts.md) · [Агенты](docs/guides/agents.md) · [Политики](docs/guides/policy.md) |
| 🚢 **Развернуть** | [Развёртывание](docs/operations/deployment.md) · [Наблюдаемость](docs/operations/observability.md) · [PostgreSQL и S3](docs/operations/postgres-and-s3.md) · [Надёжность и бэкап](docs/operations/durability.md) · [Диагностика](docs/operations/troubleshooting.md) |
| 🧩 **Строить поверх** | [AGWP v1](docs/protocol/agwp-v1.md) · [OpenAPI](docs/protocol/openapi.json) · [SDK](docs/guides/sdks.md) · [Оркестратор](docs/guides/orchestrator.md) · [Web UI](docs/guides/web-ui.md) |
| 🔬 **Разобраться** | [Архитектура](docs/architecture/overview.md) · [ADR](docs/adr/) · [Соответствие ADR реализации](docs/architecture/adr-traceability.md) · [Форматы объектов](docs/architecture/object-format.md) · [Security](docs/architecture/security.md) |

<details>
<summary><b>Все страницы по темам</b></summary>

<br>

| Тема | Страницы |
|---|---|
| Начало | [Getting started](docs/getting-started.md) · [Concepts](docs/concepts.md) |
| Руководства | [Агенты](docs/guides/agents.md) · [Политики](docs/guides/policy.md) · [Валидация](docs/guides/validation.md) · [Воркеры](docs/guides/workers.md) · [Сервер из CLI](docs/guides/remote.md) · [История](docs/guides/history.md) · [Git-мост](docs/guides/git-bridge.md) |
| Клиенты | [Web UI](docs/guides/web-ui.md) · [SDK](docs/guides/sdks.md) · [Оркестратор](docs/guides/orchestrator.md) |
| Справочник | [CLI](docs/reference/cli.md) *(генерируется)* · [AGWP v1](docs/protocol/agwp-v1.md) · [OpenAPI](docs/protocol/openapi.json) · [Форматы объектов](docs/architecture/object-format.md) |
| Эксплуатация | [Развёртывание](docs/operations/deployment.md) · [Наблюдаемость](docs/operations/observability.md) · [Релизы](docs/operations/releasing.md) · [PostgreSQL и S3](docs/operations/postgres-and-s3.md) · [Надёжность и бэкап](docs/operations/durability.md) · [GC и хранение](docs/operations/gc-and-retention.md) · [Диагностика](docs/operations/troubleshooting.md) |
| Дизайн | [Обзор](docs/architecture/overview.md) · [ADR](docs/adr/) · [Соответствие ADR реализации](docs/architecture/adr-traceability.md) |

</details>

Документацию не только пишут, но и проверяют: справочник CLI генерируется из `--help`, демо выше выполняется
в CI, имена тестов и команд из документов сверяются с кодом (`scripts/check_docs.py`), документ OpenAPI
сверяется с роутером, а утверждения о байтовых форматах подкреплены conformance-векторами.

<details>
<summary><b>Состав проекта</b></summary>

<br>

| Крейт | Роль |
|---|---|
| `ag-types` `ag-object` `ag-changeset` `ag-tree` `ag-dag` `ag-diff` `ag-merge` | детерминированное ядро: форматы, content ID, история, merge с учётом переименований |
| `ag-storage` | хранилище объектов (файлы или S3) + транзакционные метаданные (SQLite или PostgreSQL) |
| `ag-policy` · `ag-agent` | вычисление политик и подписанные аттестации · координация агентов |
| `ag-integrate` | движок: публикация, интеграция, workspaces, очередь, GC, verify, backup, blame |
| `ag-protocol` · `ag-server` · `ag-client` | AGWP v1 (protobuf + JSON-отображение) · сетевой сервер · gRPC-клиент на Rust |
| `ag-sandbox` · `ag-worker` | изолированное исполнение недоверенных проверок · воркер, который их запускает и подписывает результат |
| `ag-git` · `ag-cli` | Git-мост · бинарник `ag` (локальный и удалённый режим) |

| Каталог | Что |
|---|---|
| `apps/web` | Web UI (Vue 3 + TypeScript) |
| `sdk/typescript` · `sdk/python` | SDK, сгенерированные из документа OpenAPI и проверенные против настоящего сервера |
| `services/agent-orchestrator` | платформа агентов (Go) |
| `deploy/` | Dockerfile, Compose, Kubernetes, файлы Prometheus / Grafana / OpenTelemetry |

</details>

## Статус и честные ограничения

> [!WARNING]
> **Alpha.** Ядро и перечисленные гарантии реализованы и протестированы; так же — окружающая платформа
> (сервер, бэкенды хранилищ, воркеры, Web UI, SDK, оркестратор). В продакшене проект не использовал никто, кроме
> его авторов. Прочитайте этот раздел, прежде чем полагаться на проект.

| Область | Честная картина |
|---|---|
| 🔓 **Локальный режим** | Доверяет локальному пользователю. Идентичности — метки, а не аутентификация; кто может писать в `.ag/`, тот может обойти политику (подмена *обнаруживается* через `ag verify` и цепочку аудита). Против клиентов, которых вы не контролируете, запускайте `ag-server`: он аутентифицирует каждый запрос, и политика действует там. |
| 🖥️ **Один процесс сервера** | Контрольная база (credentials, роли, реестр репозиториев) — один файл SQLite, поэтому `ag-server` работает в одной реплике. PostgreSQL и S3 дают репозиториям надёжное, независимо масштабируемое хранилище, но не active-active сервер. Высокой доступности пока нет. |
| 🪪 **Credentials ↔ *имя* агента** | Credentials привязаны к имени агента, а не к одному запуску: два параллельных запуска одного агента могут действовать друг за друга. Оркестратор предупреждает об этом; исправление есть в [roadmap](ROADMAP.md). |
| 🚢 **Файлы развёртывания** | Проверены инструментами, но не боем. Манифесты, правила и конфиги проверяются инструментами и тестами в CI; образы контейнеров, `docker compose up`, настоящий кластер и релизный workflow на момент написания автором не запускались. [Страница про развёртывание](docs/operations/deployment.md) перечисляет, что именно проверено, а что нет. |
| 🧱 **Песочница** | `ag validate run` — **не** песочница: только для доверенного кода. Недоверенный код запускается через [`ag-worker`](docs/guides/workers.md) и его [песочницу](docs/architecture/sandbox.md): она делит ядро с хостом (это не виртуальная машина), и часть её не проверена (контейнерный бэкенд, лимиты cgroup v2, macOS) — прочитайте эту страницу (на английском) до того, как на неё полагаться. |
| 📏 **Масштаб** | Плоские манифесты деревьев: на 100 000 файлов интеграция занимает несколько сотен миллисекунд; иерархические деревья отложены с явными условиями ([ADR-0006](docs/adr/0006-flat-trees-measured.md)). Определение переименований — по файлам (без переименований каталогов и копий). |
| 🖼️ **Клиенты** | Web UI проверен только в Chromium на Linux при десктопной ширине. SDK ещё не опубликованы в npm и PyPI. Советник, ревьюеры и семантический индекс оркестратора — детерминированные эвристики, а не модели. |
| 🔒 **Аудит** | Внешнего аудита безопасности не было. |

<table>
<tr>
<td width="50%" valign="top">

**✅ Подходит для**

- команды и её агентов на одной машине, одном сервере или общем томе
- процессов с CI-гейтами
- экспериментов с автономностью агентов под политикой
- разработки на небольшом детерминированном ядре

</td>
<td width="50%" valign="top">

**⛔ Пока не подходит для**

- хостинга многих недоверяющих друг другу арендаторов
- высокой доступности
- роли единственной копии незаменимых данных. Делайте `ag backup` и держите Git-зеркало.

</td>
</tr>
</table>

## Участие

Контрибьюции приветствуются — см. [CONTRIBUTING.md](CONTRIBUTING.md) (на английском). Поскольку корректность и
есть продукт, изменения форматов, хеширования, семантики merge и протокола публикации начинаются с ADR.
Уязвимости — [SECURITY.md](SECURITY.md). Все участники следуют [Кодексу поведения](CODE_OF_CONDUCT.md).

## Лицензия

[Apache License 2.0](LICENSE). Copyright 2026 The AgentGit Authors. Если вы не указали иное, любой вклад,
сознательно отправленный вами в проект, лицензируется на тех же условиях (Apache-2.0 §5) без дополнительных
условий.
