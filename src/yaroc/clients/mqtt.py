import logging
import time
from concurrent.futures import Future
from datetime import datetime, timedelta
from typing import Any, Dict, Optional, Tuple

import paho.mqtt.client as mqtt

from ..pb.punches_pb2 import Punch, Punches
from ..pb.status_pb2 import Disconnected, MiniCallHome, Status
from ..pb.utils import create_coords_proto, create_punch_proto
from ..utils.retries import BackoffRetries
from ..utils.sim7020 import SIM7020Interface
from .client import Client

BROKER_URL = "broker.hivemq.com"
BROKER_PORT = 1883


def topics_from_mac(mac_address: str) -> Tuple[str, str, str]:
    return (
        f"yaroc/{mac_address}/p",
        f"yaroc/{mac_address}/coords",
        f"yaroc/{mac_address}/status",
    )


class MqttClient(Client):
    """Class for a simple MQTT reporting"""

    def __init__(self, mac_address: str, name: Optional[str] = None):
        def on_connect(client: mqtt.Client, userdata: Any, flags, rc: int):
            del client, userdata, flags
            logging.info(f"Connected with result code {str(rc)}")

        def on_disconnect(client: mqtt.Client, userdata: Any, rc):
            del client, userdata
            logging.error(f"Disconnected with result code {str(rc)}")

        self._message_infos: Dict[int, mqtt.MQTTMessageInfo] = {}

        def on_publish(client: mqtt.Client, userdata: Any, mid: int):
            del client, userdata
            del self._message_infos[mid]
            logging.info(f"Published id={mid}")

        self.topic_punches, self.topic_coords, self.topic_status = topics_from_mac(mac_address)

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
        self.client.connect(BROKER_URL, BROKER_PORT, 35)
        self.client.loop_start()

    def __del__(self):
        self.client.loop_stop()

    def send_punch(
        self,
        card_number: int,
        si_time: datetime,
        code: int,
        mode: int,
        process_time: datetime | None = None,
    ) -> mqtt.MQTTMessageInfo:
        punches = Punches()
        punches.punches.append(create_punch_proto(card_number, si_time, code, mode, process_time))
        punches.sending_timestamp.GetCurrentTime()
        return self._send(self.topic_punches, punches.SerializeToString())

    def send_coords(
        self, lat: float, lon: float, alt: float, timestamp: datetime
    ) -> mqtt.MQTTMessageInfo:
        coords = create_coords_proto(lat, lon, alt, timestamp)
        return self._send(self.topic_coords, coords.SerializeToString())

    def send_mini_call_home(self, mch: MiniCallHome) -> mqtt.MQTTMessageInfo:
        status = Status()
        status.mini_call_home.CopyFrom(mch)
        return self._send(self.topic_status, status.SerializeToString(), qos=0)

    def wait_for_publish(self, timeout: float | None = None):
        deadline = None if timeout is None else timeout + time.time()
        for message_info in self._message_infos.values():
            while not self.client.is_connected():
                time.sleep(1.0)

            if message_info.rc == mqtt.MQTT_ERR_SUCCESS:
                remaining = None if deadline is None else deadline - time.time()
                message_info.wait_for_publish(remaining)

    def _send(self, topic: str, message: bytes, qos: int = 1) -> mqtt.MQTTMessageInfo:
        message_info = self.client.publish(topic, message, qos=qos)
        self._message_infos[message_info.mid] = message_info
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


class SIM7020MqttClient(Client):
    """Class for an MQTT client using SIM7020's AT commands"""

    def __init__(self, mac_address: str, port: str, name: Optional[str] = None):
        self.topic_punches, self.topic_coords, self.topic_status = topics_from_mac(mac_address)
        self._at_iface = SIM7020Interface(port, name if name is not None else "SIM7020")
        self._at_iface.mqtt_connect()
        self._include_sending_timestamp = False

        self._retries = BackoffRetries(
            self._send_punch, lambda x: x, 3.0, 2.0, timedelta(minutes=10)
        )

    def _send_punches(self, punches: list[Punch]):
        punches_proto = Punches()
        for punch in punches:
            punches_proto.punches.append(punch)
        if self._include_sending_timestamp:
            punches_proto.sending_timestamp.GetCurrentTime()
        res = self._at_iface.mqtt_send(self.topic_punches, punches_proto.SerializeToString(), qos=1)
        if res:
            logging.info("Punches sent")
        else:
            logging.error("Punches not sent")
            raise Exception("Punches not sent")

    def _send_punch(self, punch: Punch):
        punches_proto = Punches()
        punches_proto.punches.append(punch)
        if self._include_sending_timestamp:
            punches_proto.sending_timestamp.GetCurrentTime()
        res = self._at_iface.mqtt_send(self.topic_punches, punches_proto.SerializeToString(), qos=1)
        if res:
            logging.info("Punches sent")
        else:
            logging.error("Punch not sent")
            raise Exception("Punch not sent")

    def send_punch(
        self,
        card_number: int,
        si_time: datetime,
        code: int,
        mode: int,
        process_time: datetime | None = None,
    ) -> Future:
        return self._retries.send(
            create_punch_proto(card_number, si_time, code, mode, process_time)
        )

    def send_coords(
        self, lat: float, lon: float, alt: float, timestamp: datetime
    ) -> mqtt.MQTTMessageInfo:
        coords = create_coords_proto(lat, lon, alt, timestamp)
        return self._send(self.topic_coords, coords.SerializeToString(), "GPS coordinates")

    def send_mini_call_home(self, mch: MiniCallHome):
        fut = self._retries.execute(self._at_iface.get_signal_dbm)
        dbm = fut.result()
        if dbm is not None:
            mch.signal_dbm = dbm

        status = Status()
        status.mini_call_home.CopyFrom(mch)
        return self._send(self.topic_status, status.SerializeToString(), "MiniCallHome", qos=0)

    def _send(self, topic: str, message: bytes, message_type: str, qos: int = 0):
        fut = self._retries.execute(self._at_iface.mqtt_send, topic, message, qos)
        if fut.result():
            logging.info(f"{message_type} sent")
        else:
            logging.error("Message not sent")
