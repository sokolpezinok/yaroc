import asyncio
import logging
import signal
import sys
import time
import tomllib
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime
from typing import List, Tuple

from ..clients.client import ClientGroup
from ..pb.status_pb2 import Status as StatusProto
from ..rs import MessageHandler, MqttConfig, SiPunchLog
from ..sources.meshtastic import MeshtasticSerial
from ..sources.mqtt import (
    MeshtasticSerialMessage,
    MeshtasticStatusMessage,
    MqttForwader,
    PunchMessage,
    StatusMessage,
)
from ..utils.container import Container, create_clients
from ..utils.status import StatusDrawer
from ..utils.sys_info import is_windows


class YarocDaemon:
    def __init__(
        self,
        dns: List[Tuple[str, str]],
        client_group: ClientGroup,
        mqtt_forwarder: MqttForwader,
        display_model: str | None = None,
        mqtt_config: MqttConfig | None = None
    ):
        self.client_group = client_group
        self.handler = MessageHandler(dns, mqtt_config)
        self.drawer = StatusDrawer(self.handler, display_model)
        self.executor = ThreadPoolExecutor(max_workers=1)

        self.mqtt_forwarder = mqtt_forwarder
        self.msh_serial = MeshtasticSerial(
            self.on_msh_status, self._handle_meshtastic_serial_mesh_packet
        )

    async def _process_punch(self, punch: SiPunchLog):
        logging.info(punch)
        await self.client_group.send_punch(punch)

    async def _handle_punches(self, msg: PunchMessage):
        try:
            punches = self.handler.punches(msg.raw, msg.mac_addr)
        except Exception as err:
            logging.error(f"Error while constructing SI punches: {err}")
            return

        tasks = [self._process_punch(punch) for punch in punches]
        await asyncio.gather(*tasks, return_exceptions=True)

    async def _handle_meshtastic_serial_mesh_packet(self, msg: MeshtasticSerialMessage):
        try:
            punches = self.handler.meshtastic_serial_mesh_packet(msg.raw)
        except Exception as err:
            logging.error(f"Error while constructing SI punch: {err}")
            return

        tasks = [self._process_punch(punch) for punch in punches]
        await asyncio.gather(*tasks, return_exceptions=True)

    async def _handle_meshtastic_serial(self, msg: MeshtasticSerialMessage):
        try:
            punches = self.handler.meshtastic_serial_service_envelope(msg.raw)
        except Exception as err:
            logging.error(f"Error while constructing SI punch: {err}")
            return

        tasks = [self._process_punch(punch) for punch in punches]
        await asyncio.gather(*tasks, return_exceptions=True)

    async def _handle_status(self, msg: StatusMessage):
        try:
            # We cannot return union types from Rust, so we have to parse the proto to detect the
            # type
            status = StatusProto.FromString(msg.raw)
        except Exception as err:
            logging.error(err)
            return

        try:
            oneof = status.WhichOneof("msg")
            self.handler.status_update(msg.raw, msg.mac_addr)
            if oneof != "disconnected":
                await self.client_group.send_status(status, f"{msg.mac_addr:0x}")
        except Exception as err:
            logging.error(f"Failed to construct proto: {err}")

    def on_msh_status(self, msg: bytes, recv_mac_addr: int):
        now = datetime.now().astimezone()
        self.handler.meshtastic_status_mesh_packet(msg, now, recv_mac_addr)

    def _handle_meshtastic_status_service_envelope(self, msg: MeshtasticStatusMessage):
        self.handler.meshtastic_status_service_envelope(msg.raw, msg.now, msg.recv_mac_addr)

    async def handle_mqtt_messages(self):
        async for msg in self.mqtt_forwarder.messages():
            if isinstance(msg, PunchMessage):
                asyncio.create_task(self._handle_punches(msg))
            elif isinstance(msg, StatusMessage):
                asyncio.create_task(self._handle_status(msg))
            elif isinstance(msg, MeshtasticStatusMessage):
                self._handle_meshtastic_status_service_envelope(msg)
            else:
                asyncio.create_task(self._handle_meshtastic_serial(msg))

    async def draw_table(self):
        await asyncio.sleep(20.0)
        while True:
            time_start = time.time()
            self.executor.submit(self.drawer.draw_status)
            await asyncio.sleep(60 - (time.time() - time_start))

    async def loop(self):
        def handle_exception(loop, context):
            msg = context.get("exception", context["message"])
            logging.error(f"Caught exception: {msg}")

        asyncio.get_event_loop().set_exception_handler(handle_exception)

        shutdown_event = asyncio.Event()

        def shutdown(signum, frame):
            signal_name = signal.Signals(signum).name
            logging.info(f"Received signal {signal_name} ({signum}). Initiating shutdown...")
            shutdown_event.set()

        signal.signal(signal.SIGTERM, shutdown)

        asyncio.create_task(self.client_group.loop())
        asyncio.create_task(self.handle_mqtt_messages())
        asyncio.create_task(self.msh_serial.loop())
        draw_task = asyncio.create_task(self.draw_table())

        try:
            await shutdown_event.wait()
        except asyncio.exceptions.CancelledError:
            logging.error("Interrupted, exiting ...")

        draw_task.cancel()
        self.drawer.clear()
        logging.info("Main loop shutting down")


async def main_loop():
    with open("yarocd.toml", "rb") as f:
        config = tomllib.load(f)
    config.pop("mqtt", None)  # Disallow MQTT forwarding to break infinite loops
    config.pop("sim7020", None)  # Disallow MQTT forwarding to break infinite loops

    container = Container()
    container.config.from_dict(config)
    container.init_resources()
    container.wire(modules=["yaroc.utils.container"])

    mac_addresses = config["mac-addresses"]
    client_group = await create_clients(container.client_factories, mac_addresses)
    if client_group.len() == 0:
        logging.info("Listening without forwarding")

    dns = [(mac_address, name) for name, mac_address in config["mac-addresses"].items()]
    meshtastic_conf = config.get("meshtastic", {})
    forwarder = MqttForwader(
        dns,
        config.get("broker_url", None),
        config.get("broker_port", None),
        meshtastic_conf.get("main_channel", None),
    )
    yaroc_daemon = YarocDaemon(dns, client_group, forwarder, config.get("display", None))
    await yaroc_daemon.loop()


if is_windows():
    from asyncio import WindowsSelectorEventLoopPolicy, set_event_loop_policy

    set_event_loop_policy(WindowsSelectorEventLoopPolicy())


def main():
    asyncio.run(main_loop())
