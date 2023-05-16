import logging
import re
import tomllib
from datetime import datetime, timedelta, timezone
from typing import Dict

import paho.mqtt.client as mqtt
from google.protobuf.timestamp_pb2 import Timestamp

from ..clients.client import Client
from ..clients.meos import MeosClient
from ..clients.roc import RocClient
from ..pb.coords_pb2 import Coordinates
from ..pb.punches_pb2 import Punches
from ..pb.status_pb2 import Status
from ..utils.script import setup_logging


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
        try:
            punches = Punches.FromString(payload)
        except Exception as err:
            logging.error(f"Error while parsing protobuf: {err}")
            return
        for punch in punches.punches:
            si_time = MqttForwader._prototime_to_datetime(punch.si_time)
            process_time = si_time + timedelta(seconds=punch.process_time_ms / 1000)
            log_message = f"{punch.card} punched {punch.code:03} at {si_time}, "
            if punches.HasField("sending_timestamp"):
                send_time = MqttForwader._prototime_to_datetime(punches.sending_timestamp)
                log_message += (
                    f"sent {send_time:%H:%M:%S.%f}, network latency "
                    f"{(now - send_time).total_seconds():6.2f}s"
                )
            else:
                log_message += (
                    f"processed {process_time:%H:%M:%S.%f}, latency "
                    f"{(now - process_time).total_seconds():6.2f}s"
                )
            log_message += f", MAC {mac_addr}"

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
        try:
            status = Status.FromString(payload)
        except Exception as err:
            logging.error(f"Error while parsing protobuf: {err}")
            return
        oneof = status.WhichOneof("msg")
        if oneof == "disconnected":
            logging.info(f"Disconnected {status.disconnected.client_name}")
        elif oneof == "mini_call_home":
            mch = status.mini_call_home
            orig_time = MqttForwader._prototime_to_datetime(mch.time)
            total_latency = now - orig_time
            if mch.freq > 0.0:
                log_message = (
                    f"At {orig_time:%H:%M:%S.%f}: {mch.cpu_temperature:5.2f}°C, "
                    f"{mch.signal_dbm:4} dBm, {mch.freq:4} MHz, "
                )
            else:
                log_message = f"At {orig_time:%H:%M:%S.%f}: {mch.codes}, "
            log_message += f"latency {total_latency.total_seconds():6.2f}s, MAC {mac_addr}"
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


with open("mqtt-forwarder.toml", "rb") as f:
    config = tomllib.load(f)

setup_logging(config)

mac_addresses = config["mac-addresses"]
meos_conf = config["client"]["meos"]
roc_conf = config["client"]["roc"]
client_map = {}
for mac_address in mac_addresses:
    clients: list[Client] = []
    if meos_conf.get("enable", True):
        logging.info("Enabled SIRAP client")
        clients.append(MeosClient(meos_conf["ip"], meos_conf["port"]))
    if roc_conf.get("enable", True):
        logging.info(f"Enabling ROC for {mac_address}")
        clients.append(RocClient(mac_address))
    if len(clients) == 0:
        logging.info(f"Listening to {mac_address} without forwarding")
    client_map[str(mac_address)] = clients

forwarder = MqttForwader(client_map)
forwarder.loop()
