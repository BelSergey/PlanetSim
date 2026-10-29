# justfile — рецепты для разработки и сборки PlanetSim.
# Запустите `just` без аргументов, чтобы увидеть список всех доступных команд.

# На Windows используем PowerShell (требуется для корректной работы рецептов вне WSL)
set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]

# Имя бинарного файла проекта
bin_name := "PlanetSim"

# Показать список всех доступных команд (запускается по умолчанию)
[private]
default:
    @just --list --unsorted

# ==============================================================================
# 🚀 Основные команды разработки
# ==============================================================================

# Запуск в режиме быстрой разработки (с динамической линковкой Bevy)
[group('dev')]
dev *args:
    cargo run --features bevy/dynamic_linking -- {{args}}

# Щадящая сборка и запуск для ПК с малым объемом ОЗУ (ограничение до 4 потоков)
[group('dev')]
dev-low-mem *args:
    cargo run --features bevy/dynamic_linking -j 4 -- {{args}}

# Стандартный запуск отладочной сборки (debug)
[group('dev')]
run *args:
    cargo run -- {{args}}

# Быстрая проверка компилируемости без генерации бинарника
[group('dev')]
check:
    cargo check --all-targets

# Сборка отладочной версии (debug)
[group('build')]
build:
    cargo build

# Оптимизированная сборка (release)
[group('build')]
release:
    cargo build --release

# Быстрая оптимизированная сборка (профиль release-thin)
[group('build')]
release-thin:
    cargo build --profile release-thin

# Очистка артефактов сборки (target/)
[group('build')]
clean:
    cargo clean

# ==============================================================================
# 🧪 Качество кода, тесты и CI
# ==============================================================================

# Запуск всех тестов (можно передать фильтр: `just test tectonics`)
[group('qa')]
test *args:
    cargo test {{args}} -- --nocapture

# Строгая проверка анализатором Clippy (предупреждения трактуются как ошибки)
[group('qa')]
lint:
    cargo clippy --all-targets --all-features -- -D warnings

# Автоматическое форматирование исходного кода через rustfmt
[group('qa')]
fmt:
    cargo fmt --all

# Проверка форматирования без изменения файлов (для CI)
[group('qa')]
fmt-check:
    cargo fmt --all -- --check

# Полный прогон проверок перед коммитом / в CI (форматирование, линтер, тесты)
[group('qa')]
ci: fmt-check lint test

# ==============================================================================
# 🛠 Настройка окружения (кроссплатформенная)
# ==============================================================================

# Полная первичная настройка окружения (системные библиотеки Bevy + компоненты Rust)
[group('setup')]
setup: setup-system setup-rust
    @echo "🎉 Окружение готово! Запустите проект командой: just dev"

# Установка обязательных системных пакетов для Bevy и линкера lld на Linux (Debian/Ubuntu)
[group('setup')]
[linux]
setup-system:
    @echo "📦 Установка системных зависимостей Bevy и линкера clang/lld (Ubuntu/Debian)..."
    sudo apt-get update
    sudo apt-get install -y \
        g++ \
        clang \
        lld \
        pkg-config \
        libx11-dev \
        libasound2-dev \
        libudev-dev \
        libwayland-dev \
        libxkbcommon-dev

# Проверка системных зависимостей на macOS
[group('setup')]
[macos]
setup-system:
    @echo "✅ На macOS дополнительные системные библиотеки не требуются (используются фреймворки Xcode)."

# Проверка системных зависимостей на Windows
[group('setup')]
[windows]
setup-system:
    @echo "✅ На Windows дополнительные пакеты не требуются (используется rust-lld из тулчейна Rust)."

# Настройка компонентов Rust (rustfmt, clippy, llvm-tools-preview)
[group('setup')]
setup-rust:
    @echo "🔧 Проверка компонентов тулчейна Rust..."
    rustup component add rustfmt clippy llvm-tools-preview
    @echo "✅ Компоненты Rust готовы."

# Установка дополнительных cargo-утилит (cargo-watch, cargo-audit, flamegraph)
[group('setup')]
setup-tools:
    #!/usr/bin/env sh
    set -eu
    echo "📦 Проверка и установка cargo-watch, cargo-audit, flamegraph..."
    for tool_pkg in "cargo-watch:cargo-watch" "cargo-audit:cargo-audit" "flamegraph:flamegraph"; do
        bin="${tool_pkg%%:*}"
        pkg="${tool_pkg#*:}"
        if command -v "$bin" >/dev/null 2>&1; then
            echo "  ✅ $pkg уже установлен"
        else
            echo "  ⬇️ Устанавливаю $pkg..."
            cargo install --locked "$pkg"
        fi
    done
    echo "✅ Все вспомогательные утилиты установлены."

# ==============================================================================
# 📊 Профилирование, документация и утилиты
# ==============================================================================

# Непрерывная проверка и запуск тестов при изменении кода (требуется cargo-watch)
[group('tools')]
watch:
    cargo watch -x check -x "test -- --nocapture"

# Сборка с оптимизациями и отладочными символами
[group('tools')]
profiling:
    cargo build --profile profiling

# Запуск профилировочной сборки
[group('tools')]
run-profiling *args:
    cargo run --profile profiling -- {{args}}

# Запись профиля выполнения и генерация flamegraph.svg (требуется flamegraph)
[group('tools')]
flamegraph:
    RUSTFLAGS='-C force-frame-pointers=y' cargo flamegraph --profile profiling --bin {{bin_name}}

# Проверка дерева зависимостей на уязвимости (требуется cargo-audit)
[group('tools')]
audit:
    cargo audit

# Генерация и открытие документации проекта в браузере
[group('tools')]
doc:
    cargo doc --no-deps --open