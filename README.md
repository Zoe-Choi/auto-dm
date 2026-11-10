# Auto DM

Instagram 게시물 댓글에 특정 키워드가 포함되면 DM을 보내기 위한 관리 앱입니다.

## 현재 구현

- 게시물별 `media id`, 키워드, DM 내용, 활성 상태 생성/수정 관리
- 키워드 쉼표 구분 다중 입력, 하나라도 포함되면 발송
- DM 발송 후 댓글에 `DM 확인해 주세요!` 또는 `DM 전송 완료!` 랜덤 답글
- 최근 Instagram 게시물 불러오기와 게시물별 자동화 추가
- 게시물 자동화 삭제와 ON/OFF 전환
- 게시물별 `팔로워에게만 발송` 옵션, 공식 API 팔로우 확인
- Instagram OAuth 연결 진입점
- Instagram Webhook 검증과 댓글 수신 엔드포인트
- 댓글 ID 기준 중복 처리 방지
- SQLite WAL 기반 큐 저장
- worker claim token으로 중복 batch 처리 방지
- 실패 작업 backoff 재시도
- PC와 모바일 폭에 맞춘 관리 화면

## 팔로우 확인 흐름

`팔로워에게만 발송`이 켜진 게시물 동작:

1. 키워드 댓글에 첫 DM 발송: `DM 받기` 버튼 포함, 댓글 공개 답글
2. 사용자가 버튼 클릭 또는 DM 답장: Webhook `messages` 수신, job 재대기
3. User Profile API `is_user_follow_business` 조회
4. 팔로우 중이면 설정된 DM 발송
5. 미팔로우면 프로필 링크와 `팔로우했어요` 버튼 발송, 클릭 시 3번부터 반복

- 댓글당 private reply는 1회만 허용, 이후 DM은 사용자 응답 후 24시간 안에만 발송
- 팔로우 여부는 사용자가 DM을 보내거나 버튼을 누른 뒤에만 조회 가능
- 프로필 링크는 프로필 열기만 함, 팔로우/언팔로우 토글 아님
- 첫 DM의 quick reply 버튼 표시 여부는 실계정 확인 필요
- 옵션을 끄면 첫 DM으로 설정된 DM 바로 발송

## 설정 화면

- 상단: 연결된 계정 또는 `Instagram 연결` 버튼, Meta 설정이 없으면 준비 안내
- `게시물`: `불러오기` 후 게시물 줄을 눌러 키워드, DM, `팔로워에게만`, `사용` 설정
- 목록에 없는 게시물은 `게시물 ID로 직접 추가`
- `최근 댓글`: 처리 상태와 오류, 실패가 있을 때만 재시도 버튼
- 모바일 폭에서 큰 터치 영역과 16px 입력창

## 운영 전 체크리스트

준비
- [x] Rust, MinGW-w64 설치 후 `cargo test` 전체 통과 확인
- [ ] Instagram 계정을 Business 또는 Creator로 전환하고 Facebook Page에 연결
- [ ] 외부 HTTPS 주소 확보: ngrok 같은 터널 또는 서버
- [ ] `.env` 작성: `APP_BASE_URL`, `META_APP_ID`, `META_APP_SECRET`, `META_REDIRECT_URI`, `META_VERIFY_TOKEN`, `SESSION_SECRET`
- [ ] 관리자 비밀번호 hash 생성 후 `ADMIN_PASSWORD_HASH` 입력

Meta 앱 설정
- [ ] Meta Developers에서 App 생성, Instagram Graph API와 Webhooks 추가
- [ ] OAuth redirect URI에 `META_REDIRECT_URI` 등록
- [ ] Webhook URL `APP_BASE_URL/webhooks/instagram`, verify token 등록
- [ ] Webhook `comments`, `messages` 필드 구독
- [ ] 권한 승인: `instagram_basic`, `instagram_manage_comments`, `instagram_manage_messages`, `pages_show_list`, `pages_read_engagement`
- [ ] 앱 검수 전에는 테스터로 등록한 계정만 동작하므로 테스트 계정 등록

실행과 연결
- [ ] `run-web.ps1`, `run-worker.ps1` 각각 실행, 운영 중 계속 켜두기
- [ ] 관리 화면에서 `Instagram 연결`, `게시물 불러오기`
- [ ] 상단에 연결 계정 표시, 준비 안내 사라짐 확인

실계정 테스트
- [ ] 팔로우 필수 끈 게시물: 키워드 댓글에 DM과 공개 답글 도착
- [ ] 키워드 없는 댓글: `키워드 없음`으로 기록, DM 미발송
- [ ] 팔로우 필수 켠 게시물: 첫 DM에 `DM 받기` 버튼 표시 여부
- [ ] 버튼이 안 보이면 아무 DM 답장으로 진행되는지 확인
- [ ] 이미 팔로우한 계정: 버튼 클릭 후 설정 DM 바로 도착
- [ ] 팔로우 안 한 계정: 팔로우 안내 도착, 팔로우 후 `팔로우했어요` 클릭 시 DM 도착
- [ ] 팔로우 직후 조회 반영 지연 여부
- [ ] 첫 DM 후 24시간 지나 버튼 클릭 시 동작
- [ ] 실패 건이 `최근 댓글`에 표시되고 재시도 버튼 동작

## 설치

Rust 버전이 기본입니다. Python 버전 `auto_dm/`은 삭제 결정 전까지 참고용으로 유지합니다. 두 버전은 같은 DB와 `.env`를 사용합니다.

1. Rust 설치: `rustup-init.exe`를 받아 아래 옵션으로 실행

```powershell
.\rustup-init.exe -y --default-host x86_64-pc-windows-gnu
```

2. MinGW-w64 설치: WinLibs `x86_64 posix seh ucrt` zip을 `%USERPROFILE%\tools`에 압축 해제
3. 실행 스크립트가 `%USERPROFILE%\.cargo\bin`, `%USERPROFILE%\tools\mingw64\bin`을 자동으로 PATH에 추가. `cargo`를 직접 쓸 때는 두 폴더를 사용자 PATH에 추가
4. `.env` 생성

```powershell
Copy-Item .env.example .env
```

- MinGW 필요 이유: `rusqlite`의 SQLite C 소스 빌드, `windows-sys`의 import library 생성에 `gcc`, `as` 필요
- 관리자 권한 없는 설치 방식, Visual Studio Build Tools 불필요

`.env` 항목:

```text
APP_BASE_URL=https://your-domain.example
DATABASE_PATH=auto_dm.sqlite3
ADMIN_USERNAME=admin
ADMIN_PASSWORD_HASH=SHA256_HASH
META_APP_ID=...
META_APP_SECRET=...
META_REDIRECT_URI=https://your-domain.example/auth/instagram/callback
META_VERIFY_TOKEN=랜덤문자열
META_GRAPH_VERSION=v20.0
```

## 실행

웹과 worker를 각각 다른 터미널에서 실행합니다. 첫 실행은 빌드 때문에 몇 분 소요됩니다.

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\run-web.ps1
powershell -ExecutionPolicy Bypass -File .\scripts\run-worker.ps1
```

- 관리 화면: `http://127.0.0.1:8000`
- 직접 실행: `cargo run --release -- web`, `cargo run --release -- worker`
- 기타 명령: `init-db`, `process-once`, `hash-password`

## Rust 코드 구조

| 파일 | 역할 | Python 대응 |
|---|---|---|
| `src/main.rs` | CLI 명령 | `cli.py` |
| `src/web.rs` | axum 웹 서버, 관리자 인증 | `app.py` |
| `src/worker.rs` | DM 발송, 팔로우 확인 | `worker.py` |
| `src/services.rs` | DB 작업 | `services.py` |
| `src/db.rs` | 스키마, migration | `db.py` |
| `src/instagram.rs` | Graph API 호출 | `instagram.py` |
| `src/security.rs` | DPAPI 토큰 암호화, `unsafe` 사용 | `security.py` |
| `src/webhook.rs` | Webhook 서명, payload 처리 | `webhook.py` |
| `src/admin.rs` | 비밀번호 hash | `admin.py` |

- 템플릿 `templates/index.html`은 Python Jinja2와 Rust minijinja 공용
- Graph API 오류 메시지에서 access token 포함 URL 제거

## Meta 설정

1. Meta Developers에서 App을 생성합니다.
2. Instagram Graph API와 Webhooks 제품을 설정합니다.
3. OAuth redirect URI에 `META_REDIRECT_URI` 값을 등록합니다.
4. Webhook callback URL을 `APP_BASE_URL/webhooks/instagram`으로 등록하고 `comments`, `messages` 필드를 구독합니다.
5. Verify token은 `.env`의 `META_VERIFY_TOKEN`과 같은 값으로 입력합니다.
6. 필요한 권한은 `instagram_basic`, `instagram_manage_comments`, `instagram_manage_messages`, `pages_show_list`, `pages_read_engagement`입니다.
7. 앱 검수와 권한 승인이 끝난 뒤 `/auth/instagram`에서 연결합니다.
8. 관리 화면에서 `게시물 불러오기`를 눌러 최근 media를 캐시합니다.
9. 게시물 카드에서 키워드와 DM 내용을 입력해 자동화를 추가합니다.

## 테스트

```powershell
cargo test
cargo clippy --all-targets
cargo fmt --check
```

- CI: GitHub Actions에서 Python, Rust 테스트 모두 실행

## Git 보안

`.env`, SQLite DB, virtualenv, `target/`, IDE 설정은 commit하지 않습니다.

## 게시물 선택 흐름

Instagram 계정은 Facebook Page에 연결된 Instagram Business/Creator 계정이어야 합니다. OAuth callback은 연결된 Page의 `instagram_business_account`와 Page access token을 저장합니다. `게시물 불러오기`는 최근 media를 로컬 DB에 캐시하고, 화면에서 게시물별 키워드와 DM을 바로 등록할 수 있게 합니다.

## 관리자 인증

외부 URL로 운영할 때는 `.env`에 `ADMIN_PASSWORD_HASH`를 설정합니다. 값이 비어 있으면 로컬 개발용으로 인증을 건너뜁니다.

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\hash-admin-password.ps1 -Password "원하는 관리자 비밀번호"
```

출력된 hash를 `ADMIN_PASSWORD_HASH`에 넣습니다. 비밀번호 원문은 git에 올리지 않습니다.

## Webhook 보안

`META_APP_SECRET`이 설정되어 있으면 Instagram Webhook POST는 `X-Hub-Signature-256` HMAC signature를 검증합니다. Webhook GET 검증은 `META_VERIFY_TOKEN`을 사용합니다.

## OAuth 보안

Instagram 연결 callback은 one-time `state` 값을 검증하고 재사용을 막습니다.

## Queue 복구

worker가 job 처리 중 종료되면 `processing` job이 남을 수 있습니다. worker는 새 batch를 claim하기 전에 15분 이상 멈춘 stale processing job을 다시 `queued`로 돌립니다.


## 실패 job 재시도

토큰/권한 문제를 해결한 뒤 `최근 댓글`의 재시도 버튼으로 `failed` job을 다시 `queued`로 돌릴 수 있습니다.


## 중복 게시물 처리

같은 media id로 자동화를 다시 추가하면 새 row를 만들지 않고 기존 자동화의 키워드, DM, 활성 상태를 업데이트합니다.
