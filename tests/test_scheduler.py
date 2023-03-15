import unittest
from datetime import datetime, timedelta

from utils.scheduler import BackoffSender


class TestScheduler(unittest.TestCase):
    def test_basic_scheduler(self):
        # Note: this is not the best test as it is non-deterministic, but the error-margin is
        # pretty wide
        stats = {2: 0, 4: 0}
        finished = {}

        def f(x: int):
            stats[x] += 1
            if stats[x] == x:
                finished[x] = datetime.now()
            else:
                raise Exception(f"Failed arg={x} for the {stats[x]}th time")

        b = BackoffSender(f, 0.1, 2.0, timedelta(minutes=0.1))
        start = datetime.now()
        print(start)
        b.send(argument=(2,))
        b.send(argument=(4,))
        b.scheduler.run()

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
