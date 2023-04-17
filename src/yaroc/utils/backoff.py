import heapq
import logging
import queue
import time
from collections.abc import Callable
from datetime import datetime, timedelta
from threading import Condition, Thread
from typing import Any, Generic, Optional, TypeVar

PRIORITY = 1


class BackoffMessageInfo:
    def __init__(self):
        self._published = False
        self._condition = Condition()

    def set_as_published(self):
        with self._condition:
            self._published = True
            self._condition.notify_all()

    def wait_for_publish(self, timeout=None):
        timeout_time = None if timeout is None else time.time() + timeout
        timeout_tenth = None if timeout is None else timeout / 10.0

        def timed_out():
            return False if timeout_time is None else time.time() > timeout_time

        with self._condition:
            while not self._published and not timed_out():
                self._condition.wait(timeout_tenth)


# TODO: make the type generic
class UnsentMessage:
    # TODO: make backoff a timedelta
    def __init__(
        self,
        argument: Any,
        deadline: datetime,
        backoff: float,
        backoff_message_info: BackoffMessageInfo,
    ):
        self.argument = argument
        self.deadline = deadline
        self.backoff = backoff
        self.backoff_message_info = backoff_message_info

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
        on_publish: Callable[[Any], Any],
        first_backoff: float,
        multiplier: float,
        max_duration: timedelta,
    ):
        self.send_function = send_function
        self.on_publish = on_publish
        self.first_backoff = first_backoff
        self.max_duration = max_duration
        self.multiplier = multiplier
        self.queue: queue.Queue[tuple[datetime, UnsentMessage]] = queue.Queue()

        self.thread = Thread(target=self._do_work)
        self.thread.daemon = True
        self.thread.start()

    def wrapped_function(self, unsent_message: UnsentMessage) -> Optional[T]:
        try:
            ret = self.send_function(unsent_message.argument)
            unsent_message.backoff_message_info.set_as_published()
            try:
                self.on_publish(unsent_message.argument)
            finally:
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
            return None

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
        backoff_message_info = BackoffMessageInfo()
        self.queue.put(
            (
                datetime.now(),
                UnsentMessage(
                    argument,
                    datetime.now() + self.max_duration,
                    self.first_backoff,
                    backoff_message_info,
                ),
            )
        )
        logging.debug("Scheduled")  # TODO: add message ID
        return backoff_message_info
