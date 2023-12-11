import asyncio
import logging
import re
import tomllib
from datetime import datetime, timezone
from typing import Dict

from aiomqtt import Client as MqttClient
from aiomqtt import Message, MqttError
from aiomqtt.types import PayloadType
from google.protobuf.timestamp_pb2 import Timestamp
from meshtastic.mesh_pb2 import Position
from meshtastic.mqtt_pb2 import ServiceEnvelope
from meshtastic.portnums_pb2 import POSITION_APP, SERIAL_APP, TELEMETRY_APP
from meshtastic.telemetry_pb2 import Telemetry

from ..clients.client import ClientGroup
from ..pb.punches_pb2 import Punches
from ..pb.status_pb2 import Status
from ..rs import SiPunch
from ..utils.container import Container, create_clients

BROKER_URL = "broker.hivemq.com"
BROKER_PORT = 1883


class MqttForwader:
    def __init__(self, client_group: ClientGroup, dns: Dict[str, str], meshtastic_mac: str):
        self.client_group = client_group
        self.dns = dns
        self.meshtastic_mac = meshtastic_mac

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

    async def _process_punch(
        self,
        punch: SiPunch,
        mac_addr: str,
        now: datetime,
        send_time: datetime | None = None,
        override_mac: str | None = None,
    ):
        log_message = (
            f"{self.dns[mac_addr]} {punch.card:7} punched {punch.code:03} "
            f"at {punch.time:%H:%M:%S.%f}, "
        )
        if send_time is None:
            log_message += f"latency {(now - punch.time).total_seconds():6.2f}s"
        else:
            log_message += (
                f"sent {send_time:%H:%M:%S.%f}, network latency "
                f"{(now - send_time).total_seconds():6.2f}s"
            )

        logging.info(log_message)
        if override_mac is not None:
            punch.mac_addr = override_mac
        await self.client_group.send_punch(punch)

    async def _handle_punches(self, mac_addr: str, payload: PayloadType, now: datetime):
        try:
            punches = Punches.FromString(MqttForwader._payload_to_bytes(payload))
        except Exception as err:
            logging.error(f"Error while parsing protobuf: {err}")
            return
        for punch in punches.punches:
            try:
                si_punch = SiPunch.from_raw(punch.raw, mac_addr)
            except Exception as err:
                logging.error(f"Error while constructing SiPunch: {err}")

            if punches.HasField("sending_timestamp"):
                send_time = MqttForwader._prototime_to_datetime(punches.sending_timestamp)
            else:
                send_time = None
            await self._process_punch(si_punch, mac_addr, now, send_time)

    async def _handle_meshtastic_serial(self, payload: PayloadType, now: datetime):
        try:
            se = ServiceEnvelope.FromString(payload)
        except Exception as err:
            logging.error(f"Error while parsing protobuf: {err}")
            return
        if not se.packet.HasField("decoded"):
            logging.error("Encrypted message! Disable encryption for meshtastic MQTT")
            return
        if se.packet.decoded.portnum != SERIAL_APP:
            logging.debug(f"Ignoring message with portnum {se.packet.decoded.portnum}")
            return
        _, node_id = se.packet.ListFields()[0]  # TODO: couldn't use `se.packet.from`
        mac_addr = f"{node_id:08x}"

        try:
            punch = SiPunch.from_raw(se.packet.decoded.payload, mac_addr)
            await self._process_punch(punch, mac_addr, now, override_mac=self.meshtastic_mac)
        except Exception as err:
            logging.error(
                f"Cannot construct SiPunch from {mac_addr} {se.packet.decoded.payload}: {err}"
            )

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
                    f"{self.dns[mac_addr]} {orig_time:%H:%M:%S}: {mch.cpu_temperature:5.2f}°C, "
                    f"{mch.signal_dbm:4}dBm, "
                )
                if mch.cellid > 0:
                    log_message += f"cell {mch.cellid:X}, "
                log_message += f"{mch.volts:3.2f}V, {mch.freq:4}MHz, "
            else:
                log_message = f"{self.dns[mac_addr]} {orig_time:%H:%M:%S}: {mch.codes}, "
            log_message += f"latency {total_latency.total_seconds():6.2f}s"
            logging.info(log_message)
            await self.client_group.send_mini_call_home(mch)

    async def _handle_meshtastic_status(self, payload: PayloadType, now: datetime):
        try:
            se = ServiceEnvelope.FromString(payload)
        except Exception as err:
            logging.error(f"Error while parsing protobuf: {err}")
            return
        if not se.packet.HasField("decoded"):
            logging.error("Encrypted message! Disable encryption for meshtastic MQTT")
            return
        _, node_id = se.packet.ListFields()[0]  # TODO: couldn't use `se.packet.from`
        mac_addr = f"{node_id:08x}"
        if se.packet.decoded.portnum == TELEMETRY_APP:
            try:
                telemetry = Telemetry.FromString(se.packet.decoded.payload)
                orig_time = datetime.fromtimestamp(telemetry.time).astimezone()
                total_latency = now - orig_time
                metrics = telemetry.device_metrics

                log_message = (
                    f"{self.dns[mac_addr]} {orig_time:%H:%M:%S}: battery {metrics.battery_level}%, "
                    f"{metrics.voltage:4.3f}V, latency {total_latency.total_seconds():6.2f}s"
                )
                logging.info(log_message)
            except Exception as err:
                logging.error(f"Error while constructing Telemetry: {err}")
        elif se.packet.decoded.portnum == POSITION_APP:
            try:
                position = Position.FromString(se.packet.decoded.payload)
                orig_time = datetime.fromtimestamp(position.time).astimezone()
                total_latency = now - orig_time
                lat, lon = position.latitude_i / 10**7, position.longitude_i / 10**7

                log_message = (
                    f"{self.dns[mac_addr]} {orig_time:%H:%M:%S}: lat {lat}, lon {lon}, "
                    f"latency {total_latency.total_seconds():6.2f}s"
                )
                logging.info(log_message)
            except Exception as err:
                logging.error(f"Error while constructing Position: {err}")

    @staticmethod
    def extract_mac(topic: str) -> str:
        match = re.match("yar/([0-9a-f]{12})/.*", topic)
        if match is None or len(match.groups()) == 0:
            logging.error(f"Invalid topic: {topic}")
            raise Exception(f"Invalid topic {topic}")

        groups = match.groups()
        return groups[0]

    async def _on_message(self, msg: Message):
        now = datetime.now().astimezone()
        topic = msg.topic.value

        try:
            if topic.endswith("/p"):
                mac_addr = MqttForwader.extract_mac(topic)
                await self._handle_punches(mac_addr, msg.payload, now)
            elif topic.endswith("/status"):
                mac_addr = MqttForwader.extract_mac(topic)
                await self._handle_status(mac_addr, msg.payload, now)
            elif topic.startswith("yar/2/c/LongFast/"):
                await self._handle_meshtastic_status(msg.payload, now)
            elif topic.startswith("yar/2/c/serial/"):
                await self._handle_meshtastic_serial(msg.payload, now)
        except Exception as err:
            logging.error(f"Failed processing message: {err}")

    async def loop(self):
        async_loop = asyncio.get_event_loop()
        asyncio.run_coroutine_threadsafe(self.client_group.loop(), async_loop)

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
                        for mac_addr in self.dns.keys():
                            await client.subscribe(f"yar/{mac_addr}/#", qos=1)
                        await client.subscribe("yar/2/c/serial/#", qos=1)
                        await client.subscribe("yar/2/c/LongFast/#", qos=1)
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

    dns = {}
    clients = create_clients(container.client_factories)
    if len(clients) == 0:
        logging.info("Listening without forwarding")
    client_group = ClientGroup(clients)

    for name, mac_address in config["mac-addresses"].items():
        dns[str(mac_address)] = name

    forwarder = MqttForwader(client_group, dns, config["meshtastic_mac"])
    asyncio.run(forwarder.loop())
