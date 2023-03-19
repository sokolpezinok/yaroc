import heapq
import logging
import queue
import threading
from collections.abc import Callable
from datetime import datetime, timedelta
from typing import Any, Generic, Optional, TypeVar

PRIORITY = 1


class UnsentMessage:
    # TODO: make backoff a timedelta
    def __init__(self, argument: Any, deadline: datetime, backoff: float):
        self.argument = argument
        self.deadline = deadline
        self.backoff = backoff

    def new_backoff(self, multiplier: float):
        self.backoff *= multiplier


T = TypeVar("T")


class BackoffSender(Generic[T]):
    """
    A sender that does exponential backoff in case of failed send operations

    The class is thread-safe
    """

    def __init__(
        self,
        send_function: Callable[[Any], T],
        callback: Callable[[Any], Any],
        first_backoff: float,
        multiplier: float,
        max_duration: timedelta,
    ):
        self.send_function = send_function
        self.callback = callback
        self.first_backoff = first_backoff
        self.max_duration = max_duration
        self.multiplier = multiplier
        self.queue = queue.Queue()

        self.thread = threading.Thread(target=self._do_work)
        self.thread.daemon = True
        self.thread.start()

    def wrapped_function(self, unsent_message: UnsentMessage) -> Optional[T]:
        try:
            ret = self.send_function(unsent_message.argument)
            self.callback(unsent_message.argument)
            return ret
        except Exception as e:
            logging.error(e)
            cur_backoff = unsent_message.backoff
            unsent_message.new_backoff(self.multiplier)
            if datetime.now() + timedelta(seconds=cur_backoff) < unsent_message.deadline:
                logging.info(f"Retrying after {cur_backoff} seconds")
                self.queue.put((datetime.now() + timedelta(seconds=cur_backoff), unsent_message))
            else:
                logging.info(f"Message expired, args = {unsent_message.argument}")

    def _do_work(self):
        messages = []
        while True:
            if len(messages) > 0:
                tim, _ = messages[0]
                timeout = max((tim - datetime.now()).total_seconds(), 0.0)
            else:
                timeout = 10000

            try:
                heapq.heappush(messages, self.queue.get(timeout=timeout))
                logging.debug("Received a new entry")
            except queue.Empty:
                (_, message) = heapq.heappop(messages)
                self.wrapped_function(message)

    def close(self, timeout=None):
        self.thread.join(timeout)

    def send(self, argument: Any):
        self.queue.put(
            (
                datetime.now(),
                UnsentMessage(argument, datetime.now() + self.max_duration, self.first_backoff),
            )
        )
        logging.debug("Scheduled")
