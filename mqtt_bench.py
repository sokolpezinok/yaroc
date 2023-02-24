import logging
from datetime import datetime
from connectors.mqtt import SimpleMqttConnector

if __name__ == "__main__":
    logging.basicConfig(
        encoding="utf-8",
        level=logging.INFO,
        format="%(asctime)s - %(levelname)s - %(message)s",
    )
    mqtt_connector = SimpleMqttConnector("spe/47")
    for i in range(10):
        mqtt_connector.send(46283, datetime.now(), datetime.now(), 53 + i, 18)
        time.sleep(2)
