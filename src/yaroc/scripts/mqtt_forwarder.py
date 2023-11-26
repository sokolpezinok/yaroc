import asyncio
import logging
import re
import tomllib
from datetime import datetime, timedelta, timezone
from typing import Dict

from aiomqtt import Client as MqttClient
from aiomqtt import Message, MqttError
from aiomqtt.types import PayloadType
from google.protobuf.timestamp_pb2 import Timestamp

from ..clients.client import ClientGroup
from ..pb.coords_pb2 import Coordinates
from ..pb.punches_pb2 import Punches
from ..pb.status_pb2 import Status
from ..utils.container import Container, create_clients
from ..utils.si import SiPunch

BROKER_URL = "broker.hivemq.com"
BROKER_PORT = 1883


class MqttForwader:
    def __init__(self, client_groups: Dict[str, ClientGroup], dns: Dict[str, str]):
        self.client_groups = client_groups
        self.dns = dns

    @staticmethod
    def _prototime_to_datetime(prototime: Timestamp) -> datetime:
        return prototime.ToDatetime().replace(tzinfo=timezone.utc).astimezone()

    @staticmethod
    def _payload_to_bytes(payload: PayloadType) -> bytes:
        if isinstance(payload, bytes):
            return payload
        elif isinstance(payload, str):
            return payload.encode("utf-8")
        else:
            raise TypeError("Unexpected type of a message payload")

    async def _handle_punches(self, mac_addr: str, payload: PayloadType, now: datetime):
        try:
            punches = Punches.FromString(MqttForwader._payload_to_bytes(payload))
        except Exception as err:
            logging.error(f"Error while parsing protobuf: {err}")
            return
        for punch in punches.punches:
            if len(punch.raw) > 0:
                si_punch = SiPunch.from_raw(punch.raw)
            else:
                si_time = MqttForwader._prototime_to_datetime(punch.si_time)
                si_punch = SiPunch.new(punch.card, punch.code, si_time, punch.mode)
            process_time = si_punch.time + timedelta(seconds=punch.process_time_ms / 1000)

            log_message = (
                f"{self.dns[mac_addr]} {si_punch.card:7} punched {si_punch.code:03} "
                f"at {si_punch.time:%H:%M:%S.%f}, "
            )
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

            logging.info(log_message)
            await self.client_groups[mac_addr].send_punch(si_punch, process_time)

    def _handle_coords(self, payload: PayloadType, now: datetime):
        coords = Coordinates.FromString(MqttForwader._payload_to_bytes(payload))
        orig_time = MqttForwader._prototime_to_datetime(coords.time)
        total_latency = now - orig_time
        log_message = (
            f"{orig_time}: {coords.latitude},{coords.longitude}, altitude "
            f"{coords.altitude}. Latency {total_latency}s."
        )
        logging.info(log_message)

    async def _handle_status(self, mac_addr: str, payload: PayloadType, now: datetime):
        try:
            status = Status.FromString(MqttForwader._payload_to_bytes(payload))
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
                    f"{self.dns[mac_addr]} {orig_time:%H:%M:%S.%f}: {mch.cpu_temperature:5.2f}°C, "
                    f"{mch.signal_dbm:4}dBm, "
                )
                if mch.cellid > 0:
                    log_message += f"cell {mch.cellid:X}, "
                log_message += f"{mch.volts:3.2f}V, {mch.freq:4}MHz, "
            else:
                log_message = f"At {orig_time:%H:%M:%S.%f}: {mch.codes}, "
            log_message += f"latency {total_latency.total_seconds():6.2f}s"
            logging.info(log_message)
            await self.client_groups[mac_addr].send_mini_call_home(mch)

    async def _on_message(self, msg: Message):
        now = datetime.now().astimezone()
        topic = msg.topic.value
        match = re.match("yaroc/([0-9a-f]{12})/.*", topic)
        if match is None:
            logging.error(f"Invalid topic: {topic}")
            return

        groups = match.groups()
        if len(groups) == 0:
            logging.debug(f"Topic {topic} doesn't match")
            return
        mac_addr = groups[0]

        if topic.endswith("/p"):
            await self._handle_punches(mac_addr, msg.payload, now)
        elif topic.endswith("/coords"):
            self._handle_coords(msg.payload, now)
        elif topic.endswith("/status"):
            await self._handle_status(mac_addr, msg.payload, now)

    async def loop(self):
        async_loop = asyncio.get_event_loop()
        for client_group in self.client_groups.values():
            asyncio.run_coroutine_threadsafe(client_group.loop(), async_loop)

        while True:
            try:
                async with MqttClient(
                    BROKER_URL,
                    BROKER_PORT,
                    timeout=15,
                    logger=logging.getLogger(),
                ) as client:
                    logging.info(f"Connected to mqtt://{BROKER_URL}")
                    async with client.messages() as messages:
                        for mac_addr in self.client_groups.keys():
                            await client.subscribe(f"yaroc/{mac_addr}/#", qos=1)
                        async for message in messages:
                            await self._on_message(message)
            except MqttError:
                logging.error(f"Connection lost to mqtt://{BROKER_URL}")
                await asyncio.sleep(5.0)
            except asyncio.exceptions.CancelledError:
                logging.error("Interrupted, exiting")
                import sys

                sys.exit(0)


def main():
    with open("mqtt-forwarder.toml", "rb") as f:
        config = tomllib.load(f)
    config.pop("mqtt", None)  # Disallow MQTT forwarding to break infinite loops
    config.pop("sim7020", None)  # Disallow MQTT forwarding to break infinite loops

    container = Container()
    container.config.from_dict(config)
    container.init_resources()
    container.wire(modules=["yaroc.utils.container"])

    client_groups = {}
    dns = {}
    for name, mac_address in config["mac-addresses"].items():
        clients = create_clients(container.client_factories, mac_address=mac_address)
        if len(clients) == 0:
            logging.info(f"Listening to {name}/{mac_address} without forwarding")
        client_groups[str(mac_address)] = ClientGroup(clients)
        dns[str(mac_address)] = name

    forwarder = MqttForwader(client_groups, dns)
    asyncio.run(forwarder.loop())
