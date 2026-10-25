#!/usr/bin/env python3
"""Validate the contents of a generated Nintendo Switch patch ZIP.

Usage: python3 tools/check_switch_zip.py path/to/Switch.zip
Does not require ROMs, keys, or the original game archives.
"""
import sys, zipfile, struct, pathlib

BUILD_ID='e43525f22282a47785aaed2dfff6e2e94d82b129'
PREFIX='Nier Automata Unburger/'

def check(filename):
    with zipfile.ZipFile(filename) as z:
        names=z.namelist()
        if len(names)!=len(set(names)): raise ValueError('Duplicate entries in ZIP')
        if any(n.startswith(PREFIX+'romfs/subtitle/') for n in names):
            raise ValueError('Unsafe loose Switch subtitle files in ZIP')
        cpk=PREFIX+'romfs/data300.cpk'
        ips=PREFIX+'exefs/'+BUILD_ID+'.ips'
        if cpk not in names or ips not in names: raise ValueError('Missing data300.cpk or ExeFS IPS')
        data=z.read(cpk); patch=z.read(ips)
        if not data.startswith(b'CPK ') or data[16:20]!=b'@UTF':
            raise ValueError('Invalid subtitle CPK header')
        if len(patch)!=65 or not patch.startswith(b'IPS32') or not patch.endswith(b'EEOF'):
            raise ValueError('Unexpected IPS32 patch')
        with open(pathlib.Path(__file__).resolve().parents[1]/'assets'/'switch_1_0_2_subtitle_priority.ips','rb') as f:
            if patch!=f.read(): raise ValueError('IPS does not match the supported game update')
        print(f'PASS: Switch layout; data300.cpk {len(data)} bytes; update-specific IPS; no loose subtitles')
        print('NOTE: This is packaging QA, NOT runtime verification of game cutscenes.')

if __name__=='__main__':
    if len(sys.argv)!=2:sys.exit('Usage: python3 tools/check_switch_zip.py path/to/Switch.zip')
    try:check(sys.argv[1])
    except (ValueError,FileNotFoundError,zipfile.BadZipFile) as e:sys.exit(f'FAIL: {e}')
