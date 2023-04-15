import logging
from datetime import datetime
from math import floor
from typing import Any, Optional

import paho.mqtt.client as mqtt
from google.protobuf.timestamp_pb2 import Timestamp

from ..pb.coords_pb2 import Coordinates
from ..pb.punches_pb2 import Punch
from ..pb.status_pb2 import Disconnected, MiniCallHome, SignalStrength, Status
from .client import Client


class SimpleMqttClient(Client):
    """Class for a simple MQTT reporting"""

    def __init__(self, mac_address: str, name: Optional[str] = None):
        def on_connect(client: mqtt.Client, userdata: Any, flags, rc: int):
            del client, userdata, flags
            logging.info(f"Connected with result code {str(rc)}")

        def on_disconnect(client: mqtt.Client, userdata: Any, rc):
            del client, userdata
            logging.error(f"Disconnected with result code {str(rc)}")

        def on_publish(client: mqtt.Client, userdata: Any, mid: int):
            del client, userdata
            logging.info(f"Published id={mid}")

        self.topic_punches = f"yaroc/{mac_address}/punches"
        self.topic_coords = f"yaroc/{mac_address}/coords"
        self.topic_status = f"yaroc/{mac_address}/status"

        disconnected = Disconnected()
        if name is None:
            disconnected.client_name = ""
            self.client = mqtt.Client()
        else:
            disconnected.client_name = str(name)
            self.client = mqtt.Client(client_id=name, clean_session=False)
        status = Status()
        status.disconnected.CopyFrom(disconnected)
        self.client.will_set(self.topic_status, status.SerializeToString(), qos=1)

        # NB-IoT is slow to connect
        self.client._connect_timeout = 35
        self.client.message_retry_set(26)
        self.client.max_inflight_messages_set(100)  # bump from 20
        self.client.enable_logger()

        self.client.on_connect = on_connect
        self.client.on_disconnect = on_disconnect
        self.client.on_publish = on_publish
        self.client.connect("broker.hivemq.com", 1883, 35)
        self.client.loop_start()

    def __del__(self):
        self.client.loop_stop()

    @staticmethod
    def _datetime_to_prototime(time: datetime) -> Timestamp:
        ret = Timestamp()
        ret.FromMilliseconds(floor(time.timestamp() * 1000))
        return ret

    def send_punch(
        self, card_number: int, si_time: datetime, now: datetime, code: int, mode: int
    ) -> mqtt.MQTTMessageInfo:
        del now
        punch = Punch()
        punch.card = card_number
        punch.code = code
        punch.mode = mode
        punch.si_time.CopyFrom(SimpleMqttClient._datetime_to_prototime(si_time))
        process_time = Timestamp()
        process_time.GetCurrentTime()
        punch.process_time.CopyFrom(process_time)
        return self._send(self.topic_punches, punch.SerializeToString())

    def send_coords(
        self, lat: float, lon: float, alt: float, timestamp: datetime
    ) -> mqtt.MQTTMessageInfo:
        coords = Coordinates()
        coords.latitude = lat
        coords.longitude = lon
        coords.altitude = alt
        coords.time.CopyFrom(SimpleMqttClient._datetime_to_prototime(timestamp))
        return self._send(self.topic_coords, coords.SerializeToString())

    def send_signal_strength(self, csq: int, orig_time: datetime) -> mqtt.MQTTMessageInfo:
        signal_strength = SignalStrength()
        signal_strength.time.CopyFrom(SimpleMqttClient._datetime_to_prototime(orig_time))
        signal_strength.csq = 20
        status = Status()
        status.signal_strength.CopyFrom(signal_strength)
        return self._send(self.topic_status, status.SerializeToString())

    def send_mini_call_home(self, mch: MiniCallHome) -> mqtt.MQTTMessageInfo:
        status = Status()
        status.mini_call_home.CopyFrom(mch)
        return self._send(self.topic_status, status.SerializeToString())

    def _send(self, topic: str, message: bytes) -> mqtt.MQTTMessageInfo:
        message_info = self.client.publish(topic, message, qos=1)
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
