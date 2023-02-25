import logging
import paho.mqtt.client as mqtt
from datetime import datetime
import time
from connectors.mqtt import SimpleMqttConnector

if __name__ == "__main__":
    logging.basicConfig(
        encoding="utf-8",
        level=logging.INFO,
        format="%(asctime)s - %(levelname)s - %(message)s",
    )
    mqtt_connector = SimpleMqttConnector("spe/47", name="benchmark")
    handles = []
    for i in range(10):
        message_info = mqtt_connector.send(
            46283, datetime.now(), datetime.now(), 53 + i, 18
        )
        handles.append(message_info)
        time.sleep(2)

    for message_info in handles:
        while not mqtt_connector.client.is_connected():
            time.sleep(2)

        if message_info.rc == mqtt.MQTT_ERR_SUCCESS:
            message_info.wait_for_publish(1)

