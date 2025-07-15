import asyncio
import logging
import signal
import time
import tomllib
from concurrent.futures import ThreadPoolExecutor
from typing import List, Tuple

from ..clients.client import ClientGroup
from ..clients.mqtt import BROKER_PORT, BROKER_URL
from ..pb.status_pb2 import Status
from ..rs import CellularLog, Message, MessageHandler, MqttConfig, SiPunchLog
from ..sources.meshtastic import MeshtasticSerial
from ..utils.container import Container, create_clients
from ..utils.status import StatusDrawer
from ..utils.sys_info import is_windows


class YarocDaemon:
    def __init__(
        self,
        dns: List[Tuple[str, str]],
        client_group: ClientGroup,
        display_model: str | None = None,
        mqtt_config: MqttConfig | None = None,
    ):
        self.client_group = client_group
        self.handler = MessageHandler(dns, mqtt_config)
        self.msh_serial = MeshtasticSerial(self.handler.msh_dev_notifier())
        self.drawer = StatusDrawer(self.handler, display_model)
        self.executor = ThreadPoolExecutor(max_workers=1)

    async def _handle_punches(self, punches: list[SiPunchLog]):
        tasks = []
        for punch in punches:
            logging.info(punch)
            tasks.append(self.client_group.send_punch(punch))
        await asyncio.gather(*tasks, return_exceptions=True)

    async def _handle_cellular_log(self, log: CellularLog):
        logging.info(log)
        proto_bytes = log.to_proto()
        if proto_bytes is not None:
            try:
                status = Status.FromString(proto_bytes)
                await self.client_group.send_status(status, log.mac_address())
            except Exception as err:
                logging.error(f"Failed to forward status: {err}")

    async def handle_messages(self):
        while True:
            msg = await self.handler.next_message()
            match msg:
                case Message.SiPunchLogs():
                    asyncio.create_task(self._handle_punches(msg[0]))
                case Message.CellularLog():
                    asyncio.create_task(self._handle_cellular_log(msg[0]))

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
        asyncio.create_task(self.handle_messages())
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
    mqtt_config = MqttConfig()
    mqtt_config.url = config.get("broker_url", BROKER_URL)
    mqtt_config.port = config.get("broker_port", BROKER_PORT)
    mqtt_config.meshtastic_channel = meshtastic_conf.get("main_channel", None)
    yaroc_daemon = YarocDaemon(dns, client_group, config.get("display", None), mqtt_config)
    await yaroc_daemon.loop()


if is_windows():
    from asyncio import WindowsSelectorEventLoopPolicy, set_event_loop_policy

    set_event_loop_policy(WindowsSelectorEventLoopPolicy())


def main():
    asyncio.run(main_loop())
