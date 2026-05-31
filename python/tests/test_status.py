import unittest
from dataclasses import dataclass
from datetime import datetime, timedelta

from yaroc.utils.status import StatusDrawer


@dataclass
class MockNodeInfo:
    name: str
    signal_strength: str
    battery_percentage: int | None
    codes: list[int]
    last_update: datetime | None
    last_punch: datetime | None


class TestStatus(unittest.TestCase):
    def test_generate_info_table(self):
        drawer = StatusDrawer()

        now = datetime.now().astimezone()

        ni1 = MockNodeInfo(
            name="Node1",
            signal_strength="-80",
            battery_percentage=95,
            codes=[100, 200, 300, 400],
            last_update=now - timedelta(seconds=30),
            last_punch=now - timedelta(minutes=5),
        )

        ni2 = MockNodeInfo(
            name="Node2",
            signal_strength="-90",
            battery_percentage=None,
            codes=[50],
            last_update=now - timedelta(minutes=65),
            last_punch=None,
        )

        table = drawer.generate_info_table([ni1, ni2])

        self.assertEqual(len(table), 2)

        # Node 1
        self.assertEqual(table[0][0], "Node1")
        self.assertEqual(table[0][1], "-80")
        self.assertEqual(table[0][2], "95")
        self.assertEqual(table[0][3], "100,200,300")  # truncated to 3 codes
        self.assertEqual(table[0][4], "now")  # < 60 seconds
        self.assertEqual(table[0][5], "5m ago")

        # Node 2
        self.assertEqual(table[1][0], "Node2")
        self.assertEqual(table[1][1], "-90")
        self.assertEqual(table[1][2], "??")
        self.assertEqual(table[1][3], "50")
        self.assertEqual(table[1][4], "1h ago")
        self.assertEqual(table[1][5], "")

    def test_draw_table(self):
        drawer = StatusDrawer()
        table = [
            ["name", "signal", "bat", "code", "last info", "last punch"],
            ["Node1", "-80", "95", "100", "now", "5m ago"],
        ]
        image = drawer.draw_table(table, 200, 100)
        self.assertEqual(image.width, 200)
        self.assertEqual(image.height, 100)

        # Test wrong number of columns
        table_bad = [["name", "signal", "bat", "code", "last info", "last punch"], ["Node1", "-80"]]
        with self.assertRaises(Exception):
            drawer.draw_table(table_bad, 200, 100)
