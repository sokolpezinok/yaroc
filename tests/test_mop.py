import unittest
import xml.etree.ElementTree as ET
from datetime import datetime, timedelta

from yaroc.clients.mop import MeosCategory, MeosCompetitor, MeosResult, MopClient

TEST_XML = """<?xml version="1.0" encoding="UTF-8"?>
<MOPComplete xmlns="http://www.melin.nu/mop" nextdifference="1377871">
  <competition date="2023-03-12" organizer="" homepage="">Training</competition>
  <ctrl id="74">74-1</ctrl>
  <ctrl id="100074">74-2</ctrl>
  <cls id="1" ord="10" radio="74,100074,200074">A</cls>
  <cls id="2" ord="40" radio="74">C</cls>
  <org id="22" nat="SVK">Klub OB Sokol Pezinok</org>
  <cmp id="10" card="2078195">
    <base org="22" cls="2" stat="1" st="360000" rt="29800" bib="47">Sara Doe</base>
    <radio>74,25220</radio>
    <input it="0" tstat="1" />
  </cmp>
  <cmp id="11" card="2111071">
    <base org="22" cls="2" stat="20" st="-1" rt="0">John Doe</base>
    <input it="0" tstat="1" />
  </cmp>
  <cmp id="12" card="2211361">
    <base org="22" cls="2" stat="4" st="375000" rt="0" bib="83">Ronald Doe</base>
    <input it="0" tstat="1"/>
  </cmp>
</MOPComplete>
"""


class TestMeos(unittest.TestCase):
    def test_competitor_parsing(self):
        xml = ET.XML(TEST_XML)
        ET.indent(xml)
        competitors = MopClient._competitors_from_meos_xml(xml)
        self.assertEqual(
            competitors[0],
            MeosCompetitor(name="Sara Doe", card=2078195, bib=47, id=10),
        )
        self.assertEqual(
            competitors[1],
            MeosCompetitor(name="John Doe", card=2111071, bib=None, id=11),
        )

    def test_result_to_xml(self):
        result = MeosResult(
            competitor=MeosCompetitor(name="Sara Doe", card=2078, bib=47, id=7),
            category=MeosCategory(name="C", id="2"),
            stat=1,
            start=timedelta(hours=10),
            time=timedelta(seconds=2980),
        )
        self.assertEqual(
            ET.tostring(MopClient._result_to_xml(result)),
            (
                b'<cmp card="2078" id="7"><base org="22" st="360000" rt="29800" cls="2" stat="1">'
                b"Sara Doe</base></cmp>"
            ),
        )

    def test_result_parsing(self):
        xml = ET.XML(TEST_XML)
        ET.indent(xml)
        results = MopClient._results_from_meos_xml(xml)
        self.assertEqual(
            results[0],
            MeosResult(
                competitor=MeosCompetitor(name="Sara Doe", card=2078195, bib=47, id=10),
                category=MeosCategory(name="C", id="2"),
                stat=1,
                start=timedelta(hours=10),
                time=timedelta(seconds=2980),
            ),
        )
        self.assertEqual(
            results[1],
            MeosResult(
                competitor=MeosCompetitor(name="John Doe", card=2111071, bib=None, id=11),
                category=MeosCategory(name="C", id="2"),
                stat=20,
                start=None,
                time=None,
            ),
        )
        self.assertEqual(
            results[2],
            MeosResult(
                competitor=MeosCompetitor(name="Ronald Doe", card=2211361, bib=83, id=12),
                category=MeosCategory(name="C", id="2"),
                stat=4,
                start=timedelta(hours=10, minutes=25),
                time=None,
            ),
        )

    def test_update_result(self):
        result = MeosResult(
            competitor=MeosCompetitor(name="Sara Doe", card=2078, bib=47, id=7),
            category=MeosCategory(name="C", id="2"),
            stat=0,
            start=timedelta(hours=10, minutes=3),
            time=None,
        )
        MopClient.update_result(result, 2, datetime(2023, 6, 9, 11, 2, 25))
        self.assertEqual(result.time, timedelta(minutes=59, seconds=25))
        self.assertEqual(result.stat, 1)

    def test_update_result_no_start(self):
        result = MeosResult(
            competitor=MeosCompetitor(name="Sara Doe", card=2078, bib=47, id=7),
            category=MeosCategory(name="C", id="2"),
            stat=0,
            start=None,
            time=None,
        )
        MopClient.update_result(result, 2, datetime(2023, 6, 9, 11, 2, 25))
        self.assertEqual(result.time, timedelta(hours=1, minutes=2, seconds=25))
        self.assertEqual(result.stat, 1)


if __name__ == "__main__":
    unittest.main()
