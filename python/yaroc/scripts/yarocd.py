import asyncio
import logging
import signal
import tomllib
from asyncio import Queue
from concurrent.futures import ThreadPoolExecutor
from typing import List, Tuple

from ..clients.client import ClientGroup
from ..clients.mqtt import BROKER_PORT, BROKER_URL
from ..pb.status_pb2 import Status
from ..rs import CellularLog, Event, MeshtasticLog, MessageHandler, MqttConfig, NodeInfo, SiPunchLog
from ..sources.usb_serial_manager import UsbSerialManager
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
        meshtastic_serial: bool = False,
        si_device_notifier: Queue[str] | None = None,
    ):
        self.client_group = client_group
        self.handler = MessageHandler(dns, mqtt_config)
        self.serial_manager = UsbSerialManager(
            self.handler.msh_dev_handler() if meshtastic_serial else None, si_device_notifier
        )
        self.drawer = StatusDrawer(display_model)
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

    async def _handle_meshtastic_log(self, log: MeshtasticLog):
        logging.info(log)

    async def handle_messages(self):
        while True:
            try:
                ev = await self.handler.next_event()
                self._handle_event(ev)
            except Exception as e:
                logging.error(f"Error while getting next message: {e}")

    def _handle_event(self, ev: Event) -> asyncio.Task | None:
        match ev:
            case Event.SiPunchLogs():
                return asyncio.create_task(self._handle_punches(ev[0]))
            case Event.CellularLog():
                return asyncio.create_task(self._handle_cellular_log(ev[0]))
            case Event.MeshtasticLog():
                return asyncio.create_task(self._handle_meshtastic_log(ev[0]))
            case Event.NodeInfos():
                return asyncio.create_task(self._draw_table(ev[0]))
        return None

    async def _draw_table(self, node_infos: list[NodeInfo]):
        self.executor.submit(self.drawer.draw_status, node_infos)

    async def loop(self):
        def handle_exception(loop, context):
            msg = context.get("exception", context["message"])
            logging.error(f"Caught exception: {msg}")

        asyncio.get_event_loop().set_exception_handler(handle_exception)

        shutdown_event = asyncio.Event()

        def shutdown(signum=None, frame=None):
            if signum is not None:
                signal_name = signal.Signals(signum).name
                logging.info(f"Received signal {signal_name} ({signum}). Initiating shutdown...")
            shutdown_event.set()

        if is_windows():
            signal.signal(signal.SIGTERM, shutdown)
            signal.signal(signal.SIGINT, shutdown)
        else:
            loop = asyncio.get_running_loop()
            for sig in (signal.SIGTERM, signal.SIGINT):
                loop.add_signal_handler(sig, shutdown)

        tasks = [
            asyncio.create_task(self.client_group.loop()),
            asyncio.create_task(self.handle_messages()),
        ]
        if self.serial_manager is not None:
            tasks.append(asyncio.create_task(self.serial_manager.loop()))

        try:
            await shutdown_event.wait()
        except asyncio.exceptions.CancelledError:
            logging.info("Interrupted, exiting ...")

        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)

        self.executor.shutdown(wait=True)
        self.drawer.clear()
        logging.info("Main loop shutting down")


async def main_loop() -> None:
    with open("yarocd.toml", "rb") as f:
        config = tomllib.load(f)

    container = Container()
    container.config.from_dict(config)
    container.init_resources()
    container.wire(modules=["yaroc.utils.container"])

    mqtt_toml_conf = config.get("mqtt", {})
    mqtt_config = MqttConfig()
    mqtt_config.url = mqtt_toml_conf.get("broker_url", BROKER_URL)
    mqtt_config.port = mqtt_toml_conf.get("broker_port", BROKER_PORT)
    if "password" in mqtt_toml_conf:
        mqtt_config.credentials = (mqtt_toml_conf["username"], mqtt_toml_conf["password"])

    mac_addresses = config.get("mac-addresses", {})
    si_device_notifier: Queue[str] | None = (
        Queue() if config.get("sportident", {}).get("watch_usb", False) else None
    )

    if "client" in config:
        config["client"].pop("mqtt", None)  # Disallow MQTT forwarding to break infinite loops
        config["client"].pop("sim7020", None)  # ... also for SIM7020
    client_group = await create_clients(
        container.client_factories, mac_addresses, si_device_notifier=si_device_notifier
    )
    if client_group.len() == 0:
        logging.info("Listening without forwarding")

    dns = [(mac_address, name) for name, mac_address in mac_addresses.items()]
    meshtastic_conf = config.get("meshtastic", {})
    mqtt_config.meshtastic_channel = meshtastic_conf.get("main_channel", None)
    meshtastic_serial = meshtastic_conf.get("watch_usb", False)
    yaroc_daemon = YarocDaemon(
        dns,
        client_group,
        config.get("display", None),
        mqtt_config,
        meshtastic_serial=meshtastic_serial,
        si_device_notifier=si_device_notifier,
    )
    await yaroc_daemon.loop()


if is_windows():
    from asyncio import WindowsSelectorEventLoopPolicy, set_event_loop_policy

    set_event_loop_policy(WindowsSelectorEventLoopPolicy())


def main():
    asyncio.run(main_loop())
