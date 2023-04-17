import time
import unittest
from datetime import datetime, timedelta

from yaroc.utils.backoff import BackoffSender


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
            raise Exception()  # Callback throws in order to test robustness

        b = BackoffSender(f, mark_finish, 0.04, 2.0, timedelta(minutes=0.1))
        start = datetime.now()
        b.send(2)
        time.sleep(0.13)
        b.send(4).wait_for_publish(2)
        published4 = datetime.now()
        b.close(0.2)

        self.assertAlmostEqual(
            finished[2].timestamp(),
            (start + timedelta(seconds=0.04)).timestamp(),
            delta=0.004,
        )
        self.assertAlmostEqual(
            finished[4].timestamp(),
            (start + timedelta(seconds=0.41)).timestamp(),
            delta=0.004,
        )
        self.assertAlmostEqual(finished[4].timestamp(), published4.timestamp(), delta=0.004)


if __name__ == "__main__":
    unittest.main()
