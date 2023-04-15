import logging
from datetime import datetime, timezone

import paho.mqtt.client as mqtt
from google.protobuf.timestamp_pb2 import Timestamp

from ..clients.client import Client
from ..clients.roc import RocClient
from ..pb.coords_pb2 import Coordinates
from ..pb.punches_pb2 import Punch
from ..pb.status_pb2 import Status


class MqttForwader:
    def __init__(self, clients: list[Client]):
        def on_connect(client: mqtt.Client, userdata, flags, rc: int):
            del userdata, flags
            logging.info(f"Connected with result code {rc}")

            # Subscribing in on_connect() means that if we lose the connection and
            # reconnect then subscriptions will be renewed.
            client.subscribe("yaroc/47/#", qos=1)

        self.clients = clients
        self.mqtt_client = mqtt.Client()
        self.mqtt_client.on_connect = on_connect
        self.mqtt_client.on_message = self._on_message
        self.mqtt_client.connect("broker.hivemq.com", 1883, 60)

    @staticmethod
    def _prototime_to_datetime(prototime: Timestamp) -> datetime:
        return prototime.ToDatetime().replace(tzinfo=timezone.utc).astimezone()

    def _handle_punch(self, payload: bytes, now: datetime):
        punch = Punch.FromString(payload)
        si_time = MqttForwader._prototime_to_datetime(punch.si_time)
        process_time = MqttForwader._prototime_to_datetime(punch.si_time)
        total_latency = now - si_time

        log_message = (
            f"{punch.code:03} dated {si_time}, processed {process_time},"
            f" latency {total_latency}"
        )
        # with open("/home/lukas/mqtt.log", "a") as f:
        #     f.write(f"{log_message}\n")
        logging.info(log_message)

        for client in self.clients:
            client.send_punch(punch.card, si_time, now, punch.code, punch.mode)

    def _handle_coords(self, payload: bytes, now: datetime):
        coords = Coordinates.FromString(payload)
        orig_time = MqttForwader._prototime_to_datetime(coords.time)
        total_latency = now - orig_time
        log_message = (
            f"{orig_time}: {coords.latitude},{coords.longitude}, altitude "
            f"{coords.altitude}. Latency {total_latency}s."
        )
        # with open("/home/lukas/events.log", "a") as f:
        #     f.write(f"{log_message}\n")
        logging.info(log_message)

    def _handle_status(self, payload: bytes, now: datetime):
        status = Status.FromString(payload)
        oneof = status.WhichOneof("msg")
        if oneof == "disconnected":
            logging.info(f"Disconnected {status.disconnected.client_name}")
        elif oneof == "signal_strength":
            signal_strength = status.signal_strength
            orig_time = MqttForwader._prototime_to_datetime(signal_strength.time)
            total_latency = now - orig_time
            csq = signal_strength.csq
            log_message = (
                f"{datetime.now()}: CSQ {csq}, {-114 + 2*csq} dBm, at {orig_time}, "
                f"latency {total_latency}"
            )
            # with open("/home/lukas/events.log", "a") as f:
            #     f.write(f"{log_message}\n")
            logging.info(log_message)
        elif oneof == "mini_call_home":
            mch = status.mini_call_home
            orig_time = MqttForwader._prototime_to_datetime(mch.time)
            total_latency = now - orig_time
            log_message = (
                f"{datetime.now()}: {mch.cpu_temperature}°C, {mch.signal_dbm} dBm, {mch.freq} MHz "
                f"at {orig_time}, latency {total_latency}"
            )
            logging.info(log_message)
            for client in self.clients:
                client.send_mini_call_home(mch)

    def _on_message(self, client, userdata, msg):
        del client, userdata
        now = datetime.now().astimezone()
        if msg.topic == "yaroc/47/punches":
            self._handle_punch(msg.payload, now)
        elif msg.topic == "yaroc/47/coords":
            self._handle_coords(msg.payload, now)
        elif msg.topic == "yaroc/47/status":
            self._handle_status(msg.payload, now)

    def loop(self):
        self.mqtt_client.loop_forever()  # Is there a way to stop this?


logging.basicConfig(
    encoding="utf-8",
    level=logging.INFO,
    format="%(asctime)s - %(levelname)s - %(message)s",
)

forwarder = MqttForwader([RocClient("klj")])
forwarder.loop()
