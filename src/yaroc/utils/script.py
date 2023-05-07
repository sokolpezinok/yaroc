import logging
from typing import Any, Dict


def setup_logging(config: Dict[str, Any]):
    log_level_config = config.get("log-level", "info").lower()
    if log_level_config == "info":
        log_level = logging.INFO
    elif log_level_config == "debug":
        log_level = logging.DEBUG
    elif log_level_config == "warn":
        log_level = logging.WARNING
    elif log_level_config == "error":
        log_level = logging.ERROR
    else:
        print(f"Wrong log-level setting {log_level_config}")
        sys.exit(1)

    logging.basicConfig(
        encoding="utf-8",
        level=log_level,
        format="%(asctime)s - %(levelname)s - %(message)s",
    )
