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

- `게시물 불러오기` 후 게시물 카드에서 키워드와 DM 입력, `설정 추가`
- 설정된 카드는 파란 테두리와 `켜짐`/`꺼짐` 표시, 같은 카드에서 수정과 삭제
- 최근 목록에 없는 게시물은 `media id로 직접 추가`
- 댓글 답글 실패는 DM 재발송 없이 `최근 댓글 처리` 메모에만 기록

## 운영 전 체크리스트

준비
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
- [ ] 운영 상태 패널의 설정 항목 모두 `설정됨` 확인

실계정 테스트
- [ ] 팔로우 필수 끈 게시물: 키워드 댓글에 DM과 공개 답글 도착
- [ ] 키워드 없는 댓글: `키워드 없음`으로 기록, DM 미발송
- [ ] 팔로우 필수 켠 게시물: 첫 DM에 `DM 받기` 버튼 표시 여부
- [ ] 버튼이 안 보이면 아무 DM 답장으로 진행되는지 확인
- [ ] 이미 팔로우한 계정: 버튼 클릭 후 설정 DM 바로 도착
- [ ] 팔로우 안 한 계정: 팔로우 안내 도착, 팔로우 후 `팔로우했어요` 클릭 시 DM 도착
- [ ] 팔로우 직후 조회 반영 지연 여부
- [ ] 첫 DM 후 24시간 지나 버튼 클릭 시 동작
- [ ] 실패 건이 `최근 댓글 처리`에 표시되고 `실패 재시도` 동작

## 설치

```powershell
cd "D:\2. Projects\auto-dm"
python -m venv .venv
.\.venv\Scripts\Activate.ps1
pip install -e ".[dev]"
Copy-Item .env.example .env
```

`.env`에 Meta App 정보를 설정합니다. 비밀번호는 넣지 않습니다.

```text
APP_BASE_URL=https://your-domain.example
DATABASE_PATH=auto_dm.sqlite3
SESSION_SECRET=change-me
ADMIN_USERNAME=admin
ADMIN_PASSWORD_HASH=<sha256 hash>
META_APP_ID=...
META_APP_SECRET=...
META_REDIRECT_URI=https://your-domain.example/auth/instagram/callback
META_VERIFY_TOKEN=랜덤문자열
META_GRAPH_VERSION=v20.0
```

## 실행

```powershell
uvicorn main:app --host 127.0.0.1 --port 8000
```

회사 PC에서 포트 접근이 막히면 서버 실행 검증은 생략하고 테스트만 실행합니다.

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
pytest
ruff check .
```

## Git 보안

`.env`, SQLite DB, virtualenv, IDE 설정은 commit하지 않습니다.

## 게시물 선택 흐름

Instagram 계정은 Facebook Page에 연결된 Instagram Business/Creator 계정이어야 합니다. OAuth callback은 연결된 Page의 `instagram_business_account`와 Page access token을 저장합니다. `게시물 불러오기`는 최근 media를 로컬 DB에 캐시하고, 화면에서 게시물별 키워드와 DM을 바로 등록할 수 있게 합니다.

## 관리자 인증

외부 URL로 운영할 때는 `.env`에 `ADMIN_PASSWORD_HASH`를 설정합니다. 값이 비어 있으면 로컬 개발용으로 인증을 건너뜁니다.

```powershell
python -c "from auto_dm.admin import hash_admin_password; print(hash_admin_password('원하는 관리자 비밀번호'))"
```

출력된 hash를 `ADMIN_PASSWORD_HASH`에 넣습니다. 비밀번호 원문은 git에 올리지 않습니다.

## Webhook 보안

`META_APP_SECRET`이 설정되어 있으면 Instagram Webhook POST는 `X-Hub-Signature-256` HMAC signature를 검증합니다. Webhook GET 검증은 `META_VERIFY_TOKEN`을 사용합니다.

## OAuth 보안

Instagram 연결 callback은 one-time `state` 값을 검증하고 재사용을 막습니다.

## Windows 실행 스크립트

집 PC에서는 repo를 pull한 뒤 `.env`를 만들고 아래 명령을 각각 다른 터미널에서 실행합니다.

```powershell
.\scripts\run-web.ps1
.\scripts\run-worker.ps1
```

관리자 비밀번호 hash는 다음 명령으로 생성합니다.

```powershell
.\scripts\hash-admin-password.ps1 -Password "원하는 관리자 비밀번호"
```

## 운영 상태 패널

첫 화면에서 관리자 인증, Meta 설정값 여부, Instagram 연결 상태, 활성 자동화 수, 게시물 캐시 수, DM job 상태를 확인할 수 있습니다. 값 자체는 노출하지 않고 설정 여부만 표시합니다.


## Queue 복구

worker가 job 처리 중 종료되면 `processing` job이 남을 수 있습니다. worker는 새 batch를 claim하기 전에 15분 이상 멈춘 stale processing job을 다시 `queued`로 돌립니다.


## 실패 job 재시도

토큰/권한 문제를 해결한 뒤 관리 화면의 `실패 재시도` 버튼으로 `failed` job을 다시 `queued`로 돌릴 수 있습니다.


## 중복 게시물 처리

같은 media id로 자동화를 다시 추가하면 새 row를 만들지 않고 기존 자동화의 키워드, DM, 활성 상태를 업데이트합니다.
