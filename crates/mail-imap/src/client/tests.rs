//! client 模块的单元测试：纯解析 + 假服务器端到端。

use std::time::Duration;

use mail_domain::account::Security;
use mail_domain::auth::AuthMaterial;
use mail_domain::error::ConnectionErrorKind;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use super::parse::{format_uid_set, parse_fetch_line, parse_list_line, parse_select_line, quote_imap_string};
use super::{ClientConfig, IdleOutcome, ImapClient};

fn config(port: u16) -> ClientConfig {
    ClientConfig {
        host: "127.0.0.1".to_string(),
        port,
        security: Security::Plain,
        username: "user@example.com".to_string(),
        auth: AuthMaterial::password("pw"),
        timeout: Duration::from_secs(5),
    }
}

async fn read_line(reader: &mut BufReader<TcpStream>) -> String {
    let mut line = String::new();
    reader.read_line(&mut line).await.expect("读命令失败");
    line.trim_end().to_string()
}

async fn greeting(socket: TcpStream) -> BufReader<TcpStream> {
    let mut reader = BufReader::new(socket);
    reader
        .get_mut()
        .write_all(b"* OK ready\r\n")
        .await
        .expect("写欢迎语");
    reader
}

#[test]
fn uid集合会合并连续区间() {
    assert_eq!(format_uid_set(&[7, 1, 2, 3, 5, 3]), "1:3,5,7");
    assert_eq!(format_uid_set(&[]), "");
}

#[test]
fn 引号串转义且拒绝换行() {
    assert_eq!(quote_imap_string("a\"b\\c").expect("成功"), "\"a\\\"b\\\\c\"");
    assert!(quote_imap_string("a\r\nb").is_err());
}

#[test]
fn 解析文件夹与选择事件() {
    let parsed = parse_list_line(b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"").expect("应解析");
    assert_eq!(parsed.full_path, "INBOX");
    assert_eq!(parsed.delimiter, "/");
    assert_eq!(parsed.attributes, vec!["\\HasNoChildren"]);

    assert!(matches!(
        parse_select_line(b"* 12 EXISTS"),
        Some(super::parse::SelectEvent::Exists(12))
    ));
    assert!(matches!(
        parse_select_line(b"* OK [UIDVALIDITY 777] valid"),
        Some(super::parse::SelectEvent::UidValidity(777))
    ));
}

#[test]
fn 解析带字面量的抓取结果() {
    let line = "{seq} 1 FETCH (UID 9 FLAGS (\\Seen) INTERNALDATE \"04-Oct-2026 08:00:00 +0800\" RFC822.SIZE 321 ENVELOPE (\"date\" \"主题\" ((\"张三\" NIL \"z\" \"example.com\")) NIL NIL ((\"李四\" NIL \"l\" \"example.com\")) NIL NIL NIL \"<m@x>\"))";
    let line = line.replace("{seq}", "*");
    let parsed = parse_fetch_line(line.as_bytes()).expect("应解析");
    assert_eq!(parsed.uid, Some(9));
    assert_eq!(parsed.flags, vec!["\\Seen"]);
    assert_eq!(parsed.size, Some(321));
    assert_eq!(parsed.envelope.as_ref().map(Vec::len), Some(10));
}

#[tokio::test]
async fn 能登录并列文件夹选收件箱() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let addr = listener.local_addr().expect("地址");
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("接受");
        let mut reader = greeting(socket).await;
        assert_eq!(
            read_line(&mut reader).await,
            "a001 LOGIN \"user@example.com\" \"pw\""
        );
        reader
            .get_mut()
            .write_all(b"a001 OK LOGIN completed\r\n")
            .await
            .expect("写");
        assert_eq!(read_line(&mut reader).await, "a002 CAPABILITY");
        reader
            .get_mut()
            .write_all(b"* CAPABILITY IMAP4rev1 ID\r\na002 OK CAPABILITY completed\r\n")
            .await
            .expect("写");
        let id = read_line(&mut reader).await;
        assert!(id.starts_with("a003 ID ("), "应发 ID：{id}");
        reader.get_mut().write_all(b"a003 OK\r\n").await.expect("写");
        assert_eq!(read_line(&mut reader).await, "a004 LIST \"\" \"*\"");
        reader
            .get_mut()
            .write_all(
                b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n* LIST (\\Sent) \"/\" \"Sent\"\r\na004 OK completed\r\n",
            )
            .await
            .expect("写");
        assert_eq!(read_line(&mut reader).await, "a005 SELECT \"INBOX\"");
        reader
            .get_mut()
            .write_all(
                b"* 3 EXISTS\r\n* OK [UIDVALIDITY 42] ok\r\n* OK [UIDNEXT 10] ok\r\na005 OK completed\r\n",
            )
            .await
            .expect("写");
    });

    let mut client = ImapClient::connect(&config(addr.port()), None)
        .await
        .expect("连接");
    assert!(client.supports("id"), "能力查询应大小写不敏感");
    let folders = client.list_folders().await.expect("列文件夹");
    assert_eq!(folders.len(), 2);
    assert_eq!(folders[0].full_path, "INBOX");
    let status = client.select("INBOX").await.expect("选择");
    assert_eq!(status.uidvalidity, Some(42));
    assert_eq!(status.uidnext, Some(10));
    assert_eq!(status.exists, 3);
}

#[tokio::test]
async fn 登录被拒会归类为认证失败() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let addr = listener.local_addr().expect("地址");
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("接受");
        let mut reader = greeting(socket).await;
        let _ = read_line(&mut reader).await;
        reader
            .get_mut()
            .write_all(b"a001 NO wrong password\r\n")
            .await
            .expect("写");
    });

    let err = ImapClient::connect(&config(addr.port()), None)
        .await
        .expect_err("应失败");
    assert_eq!(err.kind, ConnectionErrorKind::AuthFailed);
    assert!(!err.message.contains("pw"), "错误里不能出现授权码");
}

#[tokio::test]
async fn 抓取带字面量的邮件元数据() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let addr = listener.local_addr().expect("地址");
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("接受");
        let mut reader = greeting(socket).await;
        let _ = read_line(&mut reader).await;
        reader.get_mut().write_all(b"a001 OK\r\n").await.expect("写");
        let _ = read_line(&mut reader).await;
        reader
            .get_mut()
            .write_all(b"* CAPABILITY IMAP4rev1\r\na002 OK\r\n")
            .await
            .expect("写");
        assert_eq!(
            read_line(&mut reader).await,
            "a003 UID FETCH 5 (UID FLAGS INTERNALDATE RFC822.SIZE ENVELOPE)"
        );
        reader
            .get_mut()
            .write_all(
                "* 1 FETCH (UID 5 FLAGS (\\Seen) INTERNALDATE \"04-Oct-2026 08:00:00 +0800\" RFC822.SIZE 12 ENVELOPE (\"d\" {6}\r\n主题 NIL NIL NIL NIL NIL NIL NIL \"<m@x>\"))\r\na003 OK\r\n"
                    .as_bytes(),
            )
            .await
            .expect("写");
    });

    let mut client = ImapClient::connect(&config(addr.port()), None)
        .await
        .expect("连接");
    let messages = client.fetch_metadata(&[5]).await.expect("抓取");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].uid, 5);
    assert_eq!(messages[0].envelope.subject, "主题");
    assert_eq!(messages[0].size, 12);
}

#[tokio::test]
async fn 拉原文用peek命令且原始字节不乱码() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let addr = listener.local_addr().expect("地址");
    // 原始邮件里故意放字节 0xFF 与非 ASCII 文本，验证按长度读字节而不是按行解析。
    let mut raw: Vec<u8> = concat!(
        "From: a@example.com\r\n",
        "Subject: 原始\r\n",
        "\r\n",
        "正文有尾巴",
    )
    .as_bytes()
    .to_vec();
    raw.push(0xFF);
    raw.extend_from_slice(b"\r\n");
    let raw_for_server = raw.clone();

    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("接受");
        let mut reader = greeting(socket).await;
        let _ = read_line(&mut reader).await;
        reader.get_mut().write_all(b"a001 OK\r\n").await.expect("写");
        let _ = read_line(&mut reader).await;
        reader
            .get_mut()
            .write_all(b"* CAPABILITY IMAP4rev1\r\na002 OK\r\n")
            .await
            .expect("写");
        assert_eq!(read_line(&mut reader).await, "a003 UID FETCH 42 (BODY.PEEK[])");
        let header = format!("* 1 FETCH (UID 42 BODY[] {{{}}}\r\n", raw_for_server.len());
        reader.get_mut().write_all(header.as_bytes()).await.expect("写头");
        reader.get_mut().write_all(&raw_for_server).await.expect("写原文");
        reader
            .get_mut()
            .write_all(b")\r\na003 OK FETCH completed\r\n")
            .await
            .expect("写尾");
    });

    let mut client = ImapClient::connect(&config(addr.port()), None)
        .await
        .expect("连接");
    let fetched = client.fetch_body_raw(42).await.expect("拉原文");
    assert_eq!(fetched, raw);
}

#[tokio::test]
async fn 拉原文时服务器报错会归类为协议失败() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let addr = listener.local_addr().expect("地址");
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("接受");
        let mut reader = greeting(socket).await;
        let _ = read_line(&mut reader).await;
        reader.get_mut().write_all(b"a001 OK\r\n").await.expect("写");
        let _ = read_line(&mut reader).await;
        reader
            .get_mut()
            .write_all(b"* CAPABILITY IMAP4rev1\r\na002 OK\r\n")
            .await
            .expect("写");
        assert_eq!(read_line(&mut reader).await, "a003 UID FETCH 42 (BODY.PEEK[])");
        reader
            .get_mut()
            .write_all("a003 NO 邮件不存在\r\n".as_bytes())
            .await
            .expect("写");
    });

    let mut client = ImapClient::connect(&config(addr.port()), None)
        .await
        .expect("连接");
    let err = client.fetch_body_raw(42).await.expect_err("服务器拒绝应报错");
    assert_eq!(err.kind, ConnectionErrorKind::Protocol);
}
#[tokio::test]
async fn 增补搜索会过滤rfc陷阱() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let addr = listener.local_addr().expect("地址");
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("接受");
        let mut reader = greeting(socket).await;
        let _ = read_line(&mut reader).await;
        reader.get_mut().write_all(b"a001 OK\r\n").await.expect("写");
        let _ = read_line(&mut reader).await;
        reader
            .get_mut()
            .write_all(b"* CAPABILITY IMAP4rev1\r\na002 OK\r\n")
            .await
            .expect("写");
        assert_eq!(read_line(&mut reader).await, "a003 UID SEARCH UID 6:*");
        // 服务器多返回了旧的 UID 5，客户端必须过滤掉。
        reader
            .get_mut()
            .write_all(b"* SEARCH 5 6 7\r\na003 OK\r\n")
            .await
            .expect("写");
    });

    let mut client = ImapClient::connect(&config(addr.port()), None)
        .await
        .expect("连接");
    let uids = client.uid_search_after(6).await.expect("搜索");
    assert_eq!(uids, vec![6, 7]);
}

#[tokio::test]
async fn idle等到变化会返回() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let addr = listener.local_addr().expect("地址");
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("接受");
        let mut reader = greeting(socket).await;
        let _ = read_line(&mut reader).await;
        reader.get_mut().write_all(b"a001 OK\r\n").await.expect("写");
        let _ = read_line(&mut reader).await;
        reader
            .get_mut()
            .write_all(b"* CAPABILITY IMAP4rev1 IDLE\r\na002 OK\r\n")
            .await
            .expect("写");
        assert_eq!(read_line(&mut reader).await, "a003 IDLE");
        reader.get_mut().write_all(b"+ idling\r\n").await.expect("写");
        reader.get_mut().write_all(b"* 4 EXISTS\r\n").await.expect("写");
        assert_eq!(read_line(&mut reader).await, "DONE");
        reader
            .get_mut()
            .write_all(b"a003 OK IDLE terminated\r\n")
            .await
            .expect("写");
    });

    let mut client = ImapClient::connect(&config(addr.port()), None)
        .await
        .expect("连接");
    let outcome = client
        .idle_wait(Duration::from_secs(3))
        .await
        .expect("IDLE 应成功");
    assert_eq!(outcome, IdleOutcome::Changed);
}

#[tokio::test]
async fn idle超时也能正常收尾() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let addr = listener.local_addr().expect("地址");
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("接受");
        let mut reader = greeting(socket).await;
        let _ = read_line(&mut reader).await;
        reader.get_mut().write_all(b"a001 OK\r\n").await.expect("写");
        let _ = read_line(&mut reader).await;
        reader
            .get_mut()
            .write_all(b"* CAPABILITY IMAP4rev1 IDLE\r\na002 OK\r\n")
            .await
            .expect("写");
        assert_eq!(read_line(&mut reader).await, "a003 IDLE");
        reader.get_mut().write_all(b"+ idling\r\n").await.expect("写");
        // 一直不推送变化，等客户端发 DONE。
        assert_eq!(read_line(&mut reader).await, "DONE");
        reader
            .get_mut()
            .write_all(b"a003 OK IDLE terminated\r\n")
            .await
            .expect("写");
    });

    let mut client = ImapClient::connect(&config(addr.port()), None)
        .await
        .expect("连接");
    let outcome = client
        .idle_wait(Duration::from_millis(200))
        .await
        .expect("IDLE 应成功");
    assert_eq!(outcome, IdleOutcome::Timeout);
}

#[tokio::test]
async fn 追加邮件用字面量先等继续提示再发正文() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let addr = listener.local_addr().expect("地址");
    let raw = b"Subject: hi\r\n\r\nhello\r\n".to_vec();
    let expected = raw.clone();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("接受");
        let mut reader = greeting(socket).await;
        let _ = read_line(&mut reader).await;
        reader.get_mut().write_all(b"a001 OK\r\n").await.expect("写");
        let _ = read_line(&mut reader).await;
        reader
            .get_mut()
            .write_all(b"* CAPABILITY IMAP4rev1\r\na002 OK\r\n")
            .await
            .expect("写");
        assert_eq!(
            read_line(&mut reader).await,
            format!("a003 APPEND \"Sent\" (\\Seen) {{{}}}", expected.len())
        );
        reader.get_mut().write_all(b"+ go ahead\r\n").await.expect("写");
        let mut body = vec![0u8; expected.len() + 2];
        tokio::io::AsyncReadExt::read_exact(reader.get_mut(), &mut body)
            .await
            .expect("读正文");
        assert_eq!(&body[..expected.len()], expected.as_slice());
        assert_eq!(&body[expected.len()..], b"\r\n");
        reader
            .get_mut()
            .write_all(b"a003 OK APPEND completed\r\n")
            .await
            .expect("写");
    });

    let mut client = ImapClient::connect(&config(addr.port()), None)
        .await
        .expect("连接");
    client.append("Sent", &raw, true).await.expect("追加");
}

#[tokio::test]
async fn 追加邮件被服务器拒绝时报错() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let addr = listener.local_addr().expect("地址");
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("接受");
        let mut reader = greeting(socket).await;
        let _ = read_line(&mut reader).await;
        reader.get_mut().write_all(b"a001 OK\r\n").await.expect("写");
        let _ = read_line(&mut reader).await;
        reader
            .get_mut()
            .write_all(b"* CAPABILITY IMAP4rev1\r\na002 OK\r\n")
            .await
            .expect("写");
        let _ = read_line(&mut reader).await;
        reader
            .get_mut()
            .write_all(b"a003 NO quota exceeded\r\n")
            .await
            .expect("写");
    });

    let mut client = ImapClient::connect(&config(addr.port()), None)
        .await
        .expect("连接");
    let err = client
        .append("Sent", b"Subject: hi\r\n\r\nbody\r\n", true)
        .await
        .expect_err("应失败");
    assert_eq!(err.kind, ConnectionErrorKind::Protocol);
}
