# YAROC

Yet Another [ROC](https://roc.olresultat.se).

[![Python](https://github.com/sokolpezinok/yaroc/actions/workflows/python-app.yml/badge.svg)](https://github.com/sokolpezinok/yaroc/actions/workflows/python-app.yml)
[![Rust](https://github.com/sokolpezinok/yaroc/actions/workflows/rust.yml/badge.svg)](https://github.com/sokolpezinok/yaroc/actions/workflows/rust.yml)

# Installation

Clone or download this repository, then create a virtualenv and install the package.

```
python -m venv .venv
source .venv/bin/activate
pip install .
```

# Usage

```
source .venv/bin/activate
send-punch
mqtt-listener
```

# Development

In order to start developing, install also the `test` and `dev` dependencies:

```
source .venv/bin/activate
pip install ".[dev]"
pip install ".[test]"
pip install -e .
```

The last line installs the package in edit mode, so you can test each file modification immediately.
