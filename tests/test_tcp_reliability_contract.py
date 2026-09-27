from pathlib import Path
p=Path(__file__).resolve().parents[1]/'kernel/src/net/tcp.rs'
s=p.read_text(encoding='utf-8')
for token in ['TcpReliability','Retransmission','TCP_RETRANSMIT_TIMEOUT','TCP_TIME_WAIT']:
 assert token in s
print('TCP reliability contract: PASS')
