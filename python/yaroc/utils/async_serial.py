import asyncio
import itertools
import logging
import re
from asyncio import StreamReader, StreamWriter
from dataclasses import dataclass
from datetime import datetime
from typing import Awaitable, Callable, Dict, List

from serial_asyncio import open_serial_connection


@dataclass
class ATResponse:
    full_response: list[str] | str
    query: list[str] | None = None
    success: bool = False


Callback = Callable[[str], Awaitable[None]]
Coroutines = list[Awaitable[None]]


class AsyncATCom:
    def __init__(self, reader: StreamReader, writer: StreamWriter):
        self.callbacks: Dict[str, Callback] = {}
        self.delay = 0.05  # TODO: make configurable

        self._reader = reader
        self._writer = writer
        self._last_at_response = datetime.now()
        self._lock = asyncio.Lock()

    @staticmethod
    async def from_port(port: str):
        try:
            async with asyncio.timeout(10):
                reader, writer = await open_serial_connection(
                    url=port, baudrate=115200, rtscts=False
                )
                return AsyncATCom(reader, writer)
        except Exception as e:
            logging.error(e)

    def add_callback(self, prefix: str, fn: Callback):
        self.callbacks[prefix] = fn

    def match_callback(self, line: str) -> tuple[Callback, str] | None:
        for prefix, callback in self.callbacks.items():
            if line.startswith(prefix):
                return callback, line[len(prefix) :]
        return None

    def last_at_response(self) -> datetime:
        return self._last_at_response

    async def _call_until_with_timeout(
        self, command: str, timeout: float = 60, last_line: str = "OK|ERROR"
    ) -> tuple[list[str], Coroutines] | str:
        try:
            async with asyncio.timeout(timeout):
                result, coroutines = await self._call_until(command, last_line)
                self._last_at_response = datetime.now()
                return result, coroutines
        except asyncio.TimeoutError:
            return "Timed out"

    async def _call_until(
        self, command: str, last_line: str = "OK|ERROR"
    ) -> tuple[list[str], Coroutines]:
        """Call until 'last_line' matches"""
        pre_read = []
        try:
            async with asyncio.timeout(self.delay):
                while True:
                    line = (await self._reader.readline()).strip().decode("utf-8")
                    if len(line) == 0:
                        continue  # Skip empty lines
                    pre_read.append(line)
        except asyncio.TimeoutError:
            if len(pre_read) > 0:
                logging.debug(f"Read {pre_read} at the start")

        self._writer.write((command + "\r\n").encode("utf-8"))
        regex = re.compile(last_line)
        full_response: list[str] = []
        while True:
            line = (await self._reader.readline()).strip().decode("utf-8")
            if len(line) == 0:
                continue  # Skip empty lines

            full_response.append(line)
            if regex.match(line):
                break

        coroutines = []
        for line in itertools.chain(pre_read, full_response):
            ret = self.match_callback(line)
            if ret is not None:
                callback, rest = ret
                coroutines.append(callback(rest))
                continue

        return full_response, coroutines

    async def call(
        self,
        command: str,
        match: str | None = None,
        fields: List[int] = [],
        timeout: float = 20,
    ) -> ATResponse:
        async with self._lock:
            ret = await self._call_until_with_timeout(command, timeout)
            if isinstance(ret, str):
                logging.error(f"{command} failed: {ret}")
                return ATResponse("")
            full_response, coroutines = ret
        res = ATResponse(full_response)
        logging.debug(f"{command} {full_response}")
        for coro in coroutines:
            await coro

        if res.full_response[-1] == "ERROR":
            return res
        if match is None:
            res.success = True
            return res

        regex = re.compile(match)
        for line in res.full_response:
            found = regex.search(line)
            if found is None:
                continue

            res.query = []
            res.success = True
            for group in found.groups():
                assert isinstance(group, str)
                if len(fields) > 0:
                    split = group.split(",")
                    res.query = [split[field] for field in fields]
                else:
                    res.query.append(group)
            return res
        return res
