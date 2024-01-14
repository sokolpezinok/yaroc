import asyncio
import logging
import re
from datetime import datetime, timezone
from typing import Dict

from aiomqtt import Client as MqttClient
from aiomqtt import Message, MqttError
from aiomqtt.types import PayloadType
from google.protobuf.timestamp_pb2 import Timestamp
from meshtastic.mesh_pb2 import Position
from meshtastic.mqtt_pb2 import ServiceEnvelope
from meshtastic.portnums_pb2 import (
    POSITION_APP,
    RANGE_TEST_APP,
    SERIAL_APP,
    TELEMETRY_APP,
)
from meshtastic.telemetry_pb2 import Telemetry

from ..clients.client import ClientGroup
from ..pb.punches_pb2 import Punches
from ..pb.status_pb2 import Status as StatusProto
from ..rs import SiPunch
from ..utils.status import CellularRocStatus, MeshtasticRocStatus

BROKER_URL = "broker.hivemq.com"
BROKER_PORT = 1883


class MqttForwader:
    def __init__(
        self,
        client_group: ClientGroup,
        dns: Dict[str, str],
        meshtastic_mac: str,
        meshtastic_channel: str,
    ):
        self.client_group = client_group
        self.dns = dns
        self.meshtastic_mac = meshtastic_mac
        self.meshtastic_channel = meshtastic_channel
        self.cellular_status: Dict[str, CellularRocStatus] = {}
        self.meshtastic_status: Dict[str, MeshtasticRocStatus] = {}

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

    def get_cellular_status(self, mac_addr: str) -> CellularRocStatus:
        return self.cellular_status.setdefault(mac_addr, CellularRocStatus())

    def get_meshtastic_status(self, mac_addr: str) -> MeshtasticRocStatus:
        return self.meshtastic_status.setdefault(mac_addr, MeshtasticRocStatus())

    def generate_info_table(self):
        table = []
        for mac_addr, status in self.cellular_status.items():
            row = [self._resolve(mac_addr)]
            map = status.to_dict()
            row.append(map.get("dbm", ""))
            row.append(map.get("code", ""))
            row.append(map.get("last_update", ""))
            row.append(map.get("last_punch", ""))
            table.append(row)
        return table

    def _resolve(self, mac_addr: str) -> str:
        if mac_addr in self.dns:
            return self.dns[mac_addr]
        return f"MAC {mac_addr}"

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
        roc_status = self.get_cellular_status(mac_addr)
        for punch in punches.punches:
            try:
                si_punch = SiPunch.from_raw(punch.raw, mac_addr)
            except Exception as err:
                logging.error(f"Error while constructing SiPunch: {err}")
                continue

            roc_status.punch(si_punch.time, si_punch.code)
            if punches.HasField("sending_timestamp"):
                send_time = MqttForwader._prototime_to_datetime(punches.sending_timestamp)
            else:
                send_time = None
            await self._process_punch(si_punch, mac_addr, now, send_time)

    @staticmethod
    def meshtastic_mac(service_envelop: ServiceEnvelope) -> str:
        node_id = getattr(service_envelop.packet, "from")
        return f"{node_id:08x}"

    async def _handle_meshtastic_serial(self, payload: PayloadType, now: datetime):
        try:
            service_envelope = ServiceEnvelope.FromString(MqttForwader._payload_to_bytes(payload))
        except Exception as err:
            logging.error(f"Error while parsing protobuf: {err}")
            return
        packet = service_envelope.packet
        if not packet.HasField("decoded"):
            logging.error("Encrypted message! Disable encryption for meshtastic MQTT")
            return
        if packet.decoded.portnum != SERIAL_APP:
            logging.debug(f"Ignoring message with portnum {packet.decoded.portnum}")
            return

        mac_addr = MqttForwader.meshtastic_mac(service_envelope)
        try:
            punch = SiPunch.from_raw(packet.decoded.payload, mac_addr)
            await self._process_punch(punch, mac_addr, now, override_mac=self.meshtastic_mac)
        except Exception as err:
            logging.error(f"Cannot parse SiPunch from {mac_addr} {packet.decoded.payload!r}: {err}")

    async def _handle_status(self, mac_addr: str, payload: PayloadType, now: datetime):
        try:
            status = StatusProto.FromString(MqttForwader._payload_to_bytes(payload))
        except Exception as err:
            logging.error(f"Error while parsing protobuf: {err}")
            return
        oneof = status.WhichOneof("msg")
        roc_status = self.get_cellular_status(mac_addr)
        if oneof == "disconnected":
            logging.info(f"Disconnected {status.disconnected.client_name}")
            roc_status.disconnect()
        elif oneof == "mini_call_home":
            mch = status.mini_call_home
            orig_time = MqttForwader._prototime_to_datetime(mch.time)
            total_latency = now - orig_time
            log_message = f"{self._resolve(mac_addr)} {orig_time:%H:%M:%S}: "
            if mch.freq > 0.0:
                log_message += f"{mch.cpu_temperature:5.2f}°C, {mch.signal_dbm:4}dBm, "
                if mch.cellid > 0:
                    log_message += f"cell {mch.cellid:X}, "
                    roc_status.connection_state(mch.signal_dbm, mch.cellid)
                elif mch.signal_dbm != 0:
                    roc_status.connection_state(mch.signal_dbm, 0)
                log_message += f"{mch.volts:3.2f}V, {mch.freq:4}MHz, "
            else:
                log_message += f"{mch.codes}, "
            log_message += f"latency {total_latency.total_seconds():6.2f}s"
            logging.info(log_message)
            await self.client_group.send_mini_call_home(mch)

    async def _handle_meshtastic_status(self, payload: PayloadType, now: datetime):
        try:
            service_envelope = ServiceEnvelope.FromString(MqttForwader._payload_to_bytes(payload))
        except Exception as err:
            logging.error(f"Error while parsing protobuf: {err}")
            return
        if not service_envelope.packet.HasField("decoded"):
            logging.error("Encrypted message! Disable encryption for meshtastic MQTT")
            return

        mac_addr = MqttForwader.meshtastic_mac(service_envelope)
        packet = service_envelope.packet
        if packet.decoded.portnum == TELEMETRY_APP:
            try:
                telemetry = Telemetry.FromString(packet.decoded.payload)
                if not telemetry.HasField("device_metrics"):
                    return
            except Exception as err:
                logging.error(f"Error while constructing Telemetry: {err}")
                return

            orig_time = datetime.fromtimestamp(telemetry.time).astimezone()
            total_latency = now - orig_time
            metrics = telemetry.device_metrics

            log_message = (
                f"{self._resolve(mac_addr)} {orig_time:%H:%M:%S}: battery "
                f"{metrics.battery_level}%, {metrics.voltage:4.3f}V, "
                f"latency {total_latency.total_seconds():6.2f}s"
            )
            if packet.rx_rssi != 0:
                log_message += f", {packet.rx_rssi}dBm, {packet.rx_snr}SNR"
            logging.info(log_message)
        elif packet.decoded.portnum == POSITION_APP:
            if packet.to != 2**32 - 1:  # Request packets are ignored
                return
            try:
                position = Position.FromString(packet.decoded.payload)
            except Exception as err:
                logging.error(f"Error while constructing Position: {err}")
                return

            orig_time = datetime.fromtimestamp(position.time).astimezone()
            total_latency = now - orig_time
            lat, lon = position.latitude_i / 10**7, position.longitude_i / 10**7

            log_message = (
                f"{self._resolve(mac_addr)} {orig_time:%H:%M:%S}: lat {lat}, lon {lon}, "
                f"latency {total_latency.total_seconds():6.2f}s"
            )
            if packet.rx_rssi != 0:
                log_message += f", {packet.rx_rssi}dBm, {packet.rx_snr}SNR"
            logging.info(log_message)
        elif packet.decoded.portnum == RANGE_TEST_APP:
            if packet.rx_rssi == 0:
                return

            recv_time = datetime.fromtimestamp(packet.rx_time).astimezone()
            seq_number = packet.decoded.payload.decode("ascii")
            log_message = (
                f"{self._resolve(mac_addr)} {recv_time:%H:%M:%S}: range test {seq_number}, "
                f"{packet.rx_rssi}dBm, {packet.rx_snr}SNR"
            )
            logging.info(log_message)


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
            elif topic.startswith(f"yar/2/c/{self.meshtastic_channel}/"):
                await self._handle_meshtastic_status(msg.payload, now)
            elif topic.startswith("yar/2/c/serial/"):
                await self._handle_meshtastic_serial(msg.payload, now)
        except Exception as err:
            logging.error(f"Failed processing message: {err}")

    async def loop(self):
        asyncio.create_task(self.client_group.loop())

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
                        await client.subscribe(f"yar/2/c/{self.meshtastic_channel}/#", qos=1)
                        async for message in messages:
                            asyncio.create_task(self._on_message(message))
            except MqttError:
                logging.error(f"Connection lost to mqtt://{BROKER_URL}")
                await asyncio.sleep(5.0)
            except asyncio.exceptions.CancelledError:
                logging.error("Interrupted, exiting")
                import sys

                sys.exit(0)
