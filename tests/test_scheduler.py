import threading
import time
import unittest
from datetime import datetime, timedelta

from yaroc.utils.scheduler import BackoffSender


class TestScheduler(unittest.TestCase):
    def test_basic_scheduler(self):
        # Note: this is not the best test as it is non-deterministic, but the error-margin is
        # pretty wide
        stats = {2: 0, 4: 0}
        finished = {}

        def f(x: int):
            stats[x] += 1
            if stats[x] < x:
                raise Exception(f"Failed arg={x} for the {stats[x]}th time")

        def mark_finish(x: int):
            finished[x] = datetime.now()

        b = BackoffSender(f, mark_finish, 0.1, 2.0, timedelta(minutes=0.1))
        start = datetime.now()
        b.send(argument=(2,))

        b.scheduler.enter(timedelta(days=7).total_seconds(), 1, (lambda: 0), ())
        thread = threading.Thread(target=b.scheduler.run)
        thread.daemon = True
        thread.start()

        # TODO: a sleep here breaks the test. This is a hard to fix problem, probably requires a
        # redesign.
        # time.sleep(0.13)
        b.send(argument=(4,))
        thread.join(2)

        self.assertAlmostEqual(
            finished[2].timestamp(),
            (start + timedelta(seconds=0.1)).timestamp(),
            delta=0.005,
        )
        self.assertAlmostEqual(
            finished[4].timestamp(),
            (start + timedelta(seconds=0.7)).timestamp(),
            delta=0.005,
        )


if __name__ == "__main__":
    unittest.main()
