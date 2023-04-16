import logging
import time
from datetime import datetime

import paho.mqtt.client as mqtt

from ..clients.mqtt import SimpleMqttClient
from ..utils.sys_info import eth_mac_addr

logging.basicConfig(
    encoding="utf-8",
    level=logging.INFO,
    format="%(asctime)s - %(levelname)s - %(message)s",
)

mac_addr = eth_mac_addr()
assert mac_addr is not None
mqtt_client = SimpleMqttClient(mac_addr, name="benchmark")

handles = []
for i in range(1000):
    message_info = mqtt_client.send_punch(46283, datetime.now(), datetime.now(), (i + 1) % 1000, 18)
    handles.append(message_info)
    time.sleep(5)

for message_info in handles:
    # TODO: this is an implementation detail, should go inside the mqtt_client
    while not mqtt_client.client.is_connected():
        time.sleep(2)

    if message_info.rc == mqtt.MQTT_ERR_SUCCESS:
        message_info.wait_for_publish(1)
