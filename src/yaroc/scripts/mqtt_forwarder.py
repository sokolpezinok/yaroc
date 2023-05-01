import logging
import re
from datetime import datetime, timedelta, timezone
from typing import Dict

import paho.mqtt.client as mqtt
from google.protobuf.timestamp_pb2 import Timestamp

from ..clients.client import Client
from ..clients.roc import RocClient
from ..pb.coords_pb2 import Coordinates
from ..pb.punches_pb2 import Punches
from ..pb.status_pb2 import Status


class MqttForwader:
    def __init__(self, clients: Dict[str, list[Client]]):
        def on_connect(client: mqtt.Client, userdata, flags, rc: int):
            del userdata, flags
            logging.info(f"Connected with result code {rc}")

            # Subscribing in on_connect() means that if we lose the connection and
            # reconnect then subscriptions will be renewed.
            for mac_addr in clients.keys():
                client.subscribe(f"yaroc/{mac_addr}/#", qos=1)

        self.clients = clients
        self.mqtt_client = mqtt.Client()
        self.mqtt_client.on_connect = on_connect
        self.mqtt_client.on_message = self._on_message
        self.mqtt_client.connect("broker.hivemq.com", 1883, 60)

    @staticmethod
    def _prototime_to_datetime(prototime: Timestamp) -> datetime:
        return prototime.ToDatetime().replace(tzinfo=timezone.utc).astimezone()

    def _handle_punches(self, mac_addr: str, payload: bytes, now: datetime):
        punches = Punches.FromString(payload)
        for punch in punches.punches:
            si_time = MqttForwader._prototime_to_datetime(punch.si_time)
            process_time = si_time + timedelta(seconds=punch.process_time_ms / 1000)
            total_latency = now - si_time

            log_message = (
                f"{punch.code:03} dated {si_time}, processed {process_time:%H:%M:%S.%f},"
                f" latency {total_latency}"
            )
            logging.info(log_message)

            for client in self.clients[mac_addr]:
                client.send_punch(punch.card, si_time, punch.code, punch.mode, process_time)

    def _handle_coords(self, payload: bytes, now: datetime):
        coords = Coordinates.FromString(payload)
        orig_time = MqttForwader._prototime_to_datetime(coords.time)
        total_latency = now - orig_time
        log_message = (
            f"{orig_time}: {coords.latitude},{coords.longitude}, altitude "
            f"{coords.altitude}. Latency {total_latency}s."
        )
        logging.info(log_message)

    def _handle_status(self, mac_addr: str, payload: bytes, now: datetime):
        status = Status.FromString(payload)
        oneof = status.WhichOneof("msg")
        if oneof == "disconnected":
            logging.info(f"Disconnected {status.disconnected.client_name}")
        elif oneof == "mini_call_home":
            mch = status.mini_call_home
            orig_time = MqttForwader._prototime_to_datetime(mch.time)
            total_latency = now - orig_time
            log_message = (
                f"At {orig_time:%H:%M:%S.%f}: {mch.cpu_temperature}°C, {mch.signal_dbm} dBm,"
                f" {mch.freq} MHz, latency {total_latency}"
            )
            logging.info(log_message)
            for client in self.clients[mac_addr]:
                client.send_mini_call_home(mch)

    def _on_message(self, client, userdata, msg):
        del client, userdata
        now = datetime.now().astimezone()
        groups = re.match("yaroc/([0-9a-f]{12})/.*", msg.topic).groups()
        if len(groups) == 0:
            logging.debug(f"Topic {msg.topic} doesn't match")
            return
        mac_addr = groups[0]

        if msg.topic.endswith("/p"):
            self._handle_punches(mac_addr, msg.payload, now)
        elif msg.topic.endswith("/coords"):
            self._handle_coords(msg.payload, now)
        elif msg.topic.endswith("/status"):
            self._handle_status(mac_addr, msg.payload, now)

    def loop(self):
        self.mqtt_client.loop_forever()  # Is there a way to stop this?


logging.basicConfig(
    encoding="utf-8",
    level=logging.INFO,
    format="%(asctime)s - %(levelname)s - %(message)s",
)

forwarder = MqttForwader({"8c8caa504e8a": [RocClient("8c8caa504e8a")]})
forwarder.loop()
