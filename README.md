# Auto DM

Instagram 게시물 댓글에 특정 키워드가 포함되면 DM을 보내기 위한 관리 앱입니다.

## 현재 구현

- 게시물별 `media id`, 키워드, DM 내용, 활성 상태 생성/수정 관리
- 게시물 자동화 삭제와 ON/OFF 전환
- Instagram OAuth 연결 진입점
- Instagram Webhook 검증과 댓글 수신 엔드포인트
- 댓글 ID 기준 중복 처리 방지
- SQLite WAL 기반 큐 저장
- 실패 작업 backoff 재시도
- `waiting_follow` 댓글을 팔로우 확인 후 최종 DM 발송 대기로 전환
- PC와 모바일 폭에 맞춘 관리 화면

## 공식 API 제약

Meta 공식 API만 사용하면 다음 동작은 그대로 보장할 수 없습니다.

- DM 버튼 한 번으로 사용자를 즉시 팔로우시키기
- 임의 댓글 작성자가 현재 팔로워인지 실시간으로 확정 확인하기

따라서 `follow_required`가 켜진 자동화는 먼저 follow 안내 private reply를 보내고 `waiting_follow` 상태로 남깁니다. 관리자가 팔로우를 확인한 뒤 화면에서 `팔로우 확인`을 누르면 해당 댓글은 `ready_to_send`로 바뀌고 설정된 최종 DM 발송 대상이 됩니다. 완전 자동 팔로워 검증은 공식적으로 검증 가능한 수단이 확보된 뒤 확장해야 합니다.

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
4. Webhook callback URL을 `APP_BASE_URL/webhooks/instagram`으로 등록합니다.
5. Verify token은 `.env`의 `META_VERIFY_TOKEN`과 같은 값으로 입력합니다.
6. 필요한 권한은 `instagram_basic`, `instagram_manage_comments`, `instagram_manage_messages`, `pages_show_list`, `pages_read_engagement`입니다.
7. 앱 검수와 권한 승인이 끝난 뒤 `/auth/instagram`에서 연결합니다.

## 테스트

```powershell
pytest
ruff check .
```

## Git 보안

`.env`, SQLite DB, virtualenv, IDE 설정은 commit하지 않습니다.
