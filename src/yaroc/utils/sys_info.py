import psutil


def macaddr() -> str | None:
    for name, addresses in psutil.net_if_addrs().items():
        if name.startswith("e"):
            for address in addresses:
                if address.family == psutil.AF_LINK:
                    return address.address
    return None


# TODO: either return the whole struct or use an own one
def network_stats() -> tuple[int, int]:
    counters = psutil.net_io_counters()
    return (counters.bytes_sent, counters.bytes_recv)
