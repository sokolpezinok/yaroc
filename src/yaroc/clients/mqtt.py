import logging
from datetime import datetime
from math import floor
from typing import Any, Optional

import paho.mqtt.client as mqtt
from google.protobuf.timestamp_pb2 import Timestamp

from ..pb.punches_pb2 import Punch, Punches
from .client import Client


class SimpleMqttClient(Client):
    """Class for a simple MQTT reporting"""

    def __init__(self, topic: str, name: Optional[str] = None):
        def on_connect(client: mqtt.Client, userdata: Any, flags, rc: int):
            del client, userdata, flags
            logging.info(f"Connected with result code {str(rc)}")

        def on_disconnect(client: mqtt.Client, userdata: Any, rc):
            del client, userdata
            logging.error(f"Disconnected with result code {str(rc)}")

        def on_publish(client: mqtt.Client, userdata: Any, mid: int):
            del client, userdata
            logging.info(f"Published id={mid}")

        if name is None:
            self.client = mqtt.Client()
            self.client.will_set(topic, "Disconnected", qos=1)
        else:
            name = str(name)
            self.client = mqtt.Client(client_id=name, clean_session=False)
            self.client.will_set(topic, f"Disconnected {name}", qos=1)

        # NB-IoT is slow to connect
        self.client._connect_timeout = 35
        self.client.message_retry_set(26)
        self.client.max_inflight_messages_set(100)  # bump from 20
        self.client.enable_logger()
        self.topic = topic

        self.client.on_connect = on_connect
        self.client.on_disconnect = on_disconnect
        self.client.on_publish = on_publish
        self.client.connect("broker.hivemq.com", 1883, 35)
        self.client.loop_start()

    def __del__(self):
        self.client.loop_stop()

    def send_punch(
        self, card_number: int, si_time: datetime, now: datetime, code: int, mode: int
    ) -> mqtt.MQTTMessageInfo:
        del now
        punch = Punch()
        punch.card = card_number
        punch.code = code
        punch.mode = mode
        si_timestamp = Timestamp()
        si_timestamp.FromMilliseconds(floor(si_time.timestamp() * 1000))
        punch.si_time.CopyFrom(si_timestamp)
        process_time = Timestamp()
        process_time.GetCurrentTime()
        punch.process_time.CopyFrom(process_time)
        return self._send(punch.SerializeToString())

    def send_coords(
        self, lat: float, lon: float, alt: float, timestamp: datetime
    ) -> mqtt.MQTTMessageInfo:
        message = f"{lat};{lon};{alt};{timestamp}"
        return self._send(message)

    def _send(self, message: str) -> mqtt.MQTTMessageInfo:
        message_info = self.client.publish(self.topic, message, qos=1)
        if message_info.rc == mqtt.MQTT_ERR_NO_CONN:
            logging.error("Message not sent: no connection")
            # TODO: add to unsent messages
        elif message_info.rc == mqtt.MQTT_ERR_QUEUE_SIZE:
            # this should never happen as the queue size is huuuge
            logging.error("Message not sent: queue full")
        else:
            # TODO: store message_info to inquire later
            logging.info(f"Message sent, id = {message_info.mid}")
        return message_info
