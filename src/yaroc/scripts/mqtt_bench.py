import logging
import socket
import threading
import time
from datetime import datetime

import paho.mqtt.client as mqtt

from ..clients.mqtt import SimpleMqttClient

logging.basicConfig(
    encoding="utf-8",
    level=logging.INFO,
    format="%(asctime)s - %(levelname)s - %(message)s",
)

mqtt_client = SimpleMqttClient("yaroc/47", name="benchmark")


def process_gps_coords():
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.bind(("127.0.0.1", 12345))
    sock.listen(3)
    logging.debug("Waiting for connection")
    (conn, _) = sock.accept()
    logging.debug("Client connected")

    while True:
        m = conn.recv(4096)
        if m:
            logging.debug(m)
            raw_message = m.decode()
            raw_coords = raw_message.split(";")
            if len(raw_coords) == 4:
                coords = list(map(float, raw_coords[:3]))
                timestamp = datetime.fromisoformat(raw_coords[3])
                logging.info("Sending GPS coordinates")
                mqtt_client.send_coords(coords[0], coords[1], coords[2], timestamp)
        else:
            logging.debug("Nothing")
            conn.close()
            (conn, _) = sock.accept()


thread = threading.Thread(target=process_gps_coords)
thread.daemon = True
thread.start()

handles = []
for i in range(1000):
    message_info = mqtt_client.send_punch(46283, datetime.now(), datetime.now(), (i + 1) % 1000, 18)
    handles.append(message_info)
    time.sleep(5)

for message_info in handles:
    while not mqtt_client.client.is_connected():
        time.sleep(2)

    if message_info.rc == mqtt.MQTT_ERR_SUCCESS:
        message_info.wait_for_publish(1)
