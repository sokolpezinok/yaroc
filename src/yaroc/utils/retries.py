import asyncio
import logging
from collections.abc import Callable
from concurrent.futures import Future, ThreadPoolExecutor
from datetime import datetime, timedelta
from threading import Thread
from typing import Any, Generic, Optional, TypeVar

T = TypeVar("T")
A = TypeVar("A")


class BackoffRetries(Generic[A, T]):
    """
    A sender that does exponential backoff in case of failed send operations

    The class is thread-safe
    """

    def __init__(
        self,
        send_function: Callable[[A], T],
        on_publish: Callable[[Any], Any],
        first_backoff: float,
        multiplier: float,
        max_duration: timedelta,
        workers: int = 1,
    ):
        self.send_function = send_function
        self.on_publish = on_publish
        self.first_backoff = first_backoff
        self.max_duration = max_duration
        self.multiplier = multiplier
        self.executor = ThreadPoolExecutor(max_workers=workers)

        def start_background_loop(loop: asyncio.AbstractEventLoop) -> None:
            asyncio.set_event_loop(loop)
            loop.run_forever()

        self._loop = asyncio.new_event_loop()
        self._thread = Thread(target=start_background_loop, args=(self._loop,), daemon=True)
        self._thread.start()

    async def _backoff_send(self, argument: A) -> Optional[T]:
        deadline = datetime.now() + self.max_duration
        cur_backoff = self.first_backoff

        while datetime.now() < deadline:
            try:
                ret = await self._loop.run_in_executor(self.executor, self.send_function, argument)
                try:
                    self.on_publish(argument)
                finally:
                    return ret
            except Exception as e:
                logging.error(e)
                if datetime.now() + timedelta(seconds=cur_backoff) >= deadline:
                    cur_backoff = (deadline - datetime.now()).total_seconds()
                logging.info(f"Retrying after {cur_backoff} seconds")
                await asyncio.sleep(cur_backoff)
                cur_backoff = cur_backoff * self.multiplier

        logging.info(f"Message expired, args = {argument}")
        return None

    def close(self, timeout=None):
        self._thread.join(timeout)

    def send(self, argument: A) -> Future:
        future = asyncio.run_coroutine_threadsafe(self._backoff_send(argument), self._loop)
        logging.debug("Scheduled")  # TODO: add message ID
        return future

    def execute(self, fn, *args) -> Future:
        return self.executor.submit(fn, *args)
