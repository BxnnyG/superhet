"""Generate the hand-built welle-cli mux.json fixtures (shape from welle.io 2.4 jsonconvert.cpp)."""
import json
import sys

out = sys.argv[1]


def label(text, short=None):
    return {"label": text.ljust(16), "shortlabel": short or text[:8], "fig2label": "", "fig2rfu": False, "fig2charset": "Undefined"}


def comp(nr, tm, asc, bitrate, prot="EEP 3-A", primary=True, label_text=""):
    return {"componentnr": nr, "primary": primary, "caflag": False, "transportmode": tm, "label": label(label_text),
            "subchannel": {"subchid": nr, "bitrate": bitrate, "cu": 54, "sad": 0, "protection": prot, "language": 0, "languagestring": "unknown"},
            "scid": None, "ascty": asc, "dscty": None}


def svc(sid, name, pty, ptys, comps, dls=None, mot=0, errors=(0, 0, 0), mp3=True):
    playing = dls is not None
    return {"sid": "0x" + sid.lower(), "programType": pty, "ptystring": ptys, "language": 8, "languagestring": "German", "label": label(name),
            "components": comps, "channels": 2 if playing else 0, "samplerate": 48000 if playing else 0, "mode": "AAC" if playing else "invalid",
            "mot": {"time": 1790000100 if mot else 0, "lastchange": mot},
            "dls": {"label": dls or "", "time": 1790000100 if dls else 0, "lastchange": mot},
            "errorcounters": {"frameerrors": errors[0], "rserrors": errors[1], "aacerrors": errors[2], "time": 1790000100},
            "xpaderror": {"haserror": False}, "url_mp3": f"/mp3/0x{sid.lower()}" if mp3 else None,
            "audiolevel": {"time": 1790000100, "left": 1200, "right": 1100} if playing else None}


receiver = {"hardware": {"name": "rtl_tcp_client (server: 127.0.0.1:41233)", "gain": 0.0},
            "software": {"name": "welle.io", "version": "2.4+ds", "fftwindowplacement": "EarliestPeakWithBinning",
                         "coarsecorrectorenabled": True, "freqsyncmethod": "GetPhaseCorrelationOffset", "lastchannelchange": 1790000000000}}
doc = {
    "receiver": receiver,
    "ensemble": {"label": label("DR Deutschland", "DRDE"), "id": "0xd210", "ecc": "0xe0"},
    "services": [
        svc("D210", "Deutschlandfunk", 1, "Information", [comp(0, "audio", "DAB+", 72)],
            dls="Nachrichten: Bundestag debattiert", mot=1790000000, errors=(1, 2, 0)),
        svc("D212", "DKultur", 8, "Culture", [comp(1, "audio", "DAB", 128, prot="UEP 3")]),
        svc("E0D2FF01", "Journaline", 0, "None", [dict(comp(2, "packetdata", None, 0), scid=7)], mp3=False),
    ],
    "utctime": {"year": 2026, "month": 10, "day": 27, "hour": 12, "minutes": 3, "lto": 2.0},
    "messages": [], "tii": [], "cir_peaks": [],
    "demodulator": {"fic": {"numcrcerrors": 2}, "time_last_fct0_frame": 1790000099000, "snr": 14.5, "frequencycorrection": -310.0},
}
empty = {
    "receiver": receiver,
    "ensemble": {"label": label(""), "id": "0x0000", "ecc": "0x00"},
    "services": [],
    "utctime": {"year": 0, "month": 0, "day": 0, "hour": 0, "minutes": 0, "lto": 0.0},
    "messages": [], "tii": [], "cir_peaks": [],
    "demodulator": {"fic": {"numcrcerrors": 0}, "time_last_fct0_frame": 0, "snr": 0.0, "frequencycorrection": 0.0},
}
json.dump(doc, open(f"{out}/mux-locked.json", "w"), indent=1)
json.dump(empty, open(f"{out}/mux-nosignal.json", "w"), indent=1)
