import { useCallback, useState, type ReactNode } from 'react'
import { AuthPage } from '../features/auth/AuthPage'
import { Modal } from './Modal'

type Page = 'home' | 'access' | 'cameras' | 'calls' | 'events' | 'settings'
type CallState = 'incoming' | 'active' | 'recent'
type Confirmation = {
  title: string
  copy: string
  action: string
  success: string
  onConfirm: () => void
}

const navigation: Array<{ page: Page; label: string; glyph: string }> = [
  { page: 'home', label: 'Главная', glyph: '⌂' },
  { page: 'access', label: 'Доступ', glyph: '⌑' },
  { page: 'cameras', label: 'Камеры', glyph: '◉' },
  { page: 'calls', label: 'Звонки', glyph: '☎' },
  { page: 'events', label: 'События', glyph: '✓' },
]

const accessPoints = [
  { name: 'Подъезд', detail: 'Основной вход' },
  { name: 'Калитка', detail: 'Со стороны двора' },
  { name: 'Паркинг', detail: 'Шлагбаум' },
]

function Glyph({ children }: { children: ReactNode }) {
  return <span className="glyph" aria-hidden="true">{children}</span>
}

function CameraScene({ archive = false }: { archive?: boolean }) {
  return <div className="camera-scene" aria-label={archive ? 'Архив камеры у подъезда' : 'Прямая камера у подъезда'}>
    <div className="scene-building"><i /><i /><i /><i /><i /><i /></div>
    <div className="scene-door" />
    <span className="live-badge"><i />{archive ? 'АРХИВ' : 'LIVE'}</span>
    <span className="camera-time">10:14:26</span>
    <button className="play-button" aria-label="Запустить прямой эфир" type="button">▶</button>
    <div className="camera-caption"><span><b>Камера у подъезда</b><small>{archive ? 'Сегодня · 09:42' : 'Синтетический видеопоток'}</small></span></div>
  </div>
}

function App() {
  const [page, setPage] = useState<Page>('home')
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [flatOpen, setFlatOpen] = useState(false)
  const [flat, setFlat] = useState('Демо-квартира · Подъезд 1')
  const [archive, setArchive] = useState(false)
  const [authOpen, setAuthOpen] = useState(false)
  const [call, setCall] = useState<CallState>('incoming')
  const [muted, setMuted] = useState(false)
  const [cameraOn, setCameraOn] = useState(true)
  const [soundOn, setSoundOn] = useState(true)
  const [speaker, setSpeaker] = useState<'browser' | 'system'>('browser')

  const closeConfirmation = useCallback(() => setConfirmation(null), [])
  const closeAuth = useCallback(() => setAuthOpen(false), [])

  function showNotice(message: string) {
    setNotice(message)
    window.setTimeout(() => setNotice(null), 4200)
  }

  function askCommand(title: string, action: string, onConfirm: () => void, success = 'Команда применена только в безопасном предпросмотре') {
    setConfirmation({
      title,
      action,
      onConfirm,
      success,
      copy: 'В рабочем режиме команда была бы отправлена только после этого подтверждения. Preview ничего не отправляет.',
    })
  }

  function askDoor(name: string, duringCall = false) {
    askCommand(
      duringCall ? 'Открыть дверь во время звонка?' : `Открыть ${name}?`,
      duringCall ? 'Подтвердить открытие двери' : 'Подтвердить открытие',
      () => undefined,
      'Команда не отправлена: это безопасный предпросмотр UI',
    )
  }

  function executeConfirmation() {
    if (!confirmation) return
    const command = confirmation
    setConfirmation(null)
    command.onConfirm()
    showNotice(command.success)
  }

  function selectPage(next: Page) {
    setPage(next)
    setFlatOpen(false)
  }

  const accessList = <div className="access-list">
    {accessPoints.map((point) => <div className="access-item" key={point.name}>
      <span className="access-icon"><Glyph>⌑</Glyph></span>
      <span className="access-copy"><b>{point.name}</b><small><i className="online" />{point.detail}</small></span>
      <button className="open-button" aria-label={`Открыть ${point.name}`} onClick={() => askDoor(point.name)} type="button">⌑ Открыть</button>
    </div>)}
  </div>

  const dashboard = <>
    <section className="page-heading"><div><p className="eyebrow">Синтетические данные · Четверг, 6 августа</p><h1>Домофон</h1><p>Камеры, двери и звонки — в одном месте.</p></div><button className="outline-button" type="button"><span className="status-dot" />Все устройства на связи</button></section>
    <section className="hero-grid" aria-label="Камера и быстрый доступ">
      <article className="camera-card"><CameraScene /></article>
      <aside className="quick-panel"><div className="section-title-row"><div><span className="eyebrow">Быстрый доступ</span><h2>Точки входа</h2></div><span className="count-badge">3</span></div>{accessList}<button className="call-strip" onClick={() => selectPage('calls')} type="button"><Glyph>☎</Glyph><span><b>{call === 'incoming' ? 'Входящий вызов' : 'Нет активного вызова'}</b><small>{call === 'incoming' ? 'Подъезд · ждёт ответа' : 'Откройте журнал звонков'}</small></span><Glyph>›</Glyph></button></aside>
    </section>
    <section className="lower-grid"><article className="panel"><div className="section-title-row"><div><span className="eyebrow">Журнал</span><h2>Последние события</h2></div><button className="text-button" onClick={() => selectPage('events')} type="button">Все события ›</button></div>{['Дверь открыта · Подъезд', 'Входящий вызов · 18 секунд', 'Движение у входа'].map((event, index) => <button className="event-row" onClick={() => selectPage(index === 1 ? 'calls' : 'events')} type="button" key={event}><span className="event-art"><Glyph>{index === 1 ? '☎' : index === 2 ? '◉' : '⌑'}</Glyph></span><span><b>{event}</b><small>{index === 0 ? 'Сегодня, 09:42' : 'Вчера, 20:17'}</small></span><Glyph>›</Glyph></button>)}</article><article className="panel notification-card"><span className="connection-orbit"><Glyph>●</Glyph></span><span className="eyebrow">Уведомления</span><h2>Не пропускайте звонки</h2><p>Браузер может показать входящий вызов, когда вкладка свёрнута.</p><button className="primary-button" onClick={() => selectPage('settings')} type="button">Настроить</button></article></section>
  </>

  const access = <><section className="page-heading"><div><p className="eyebrow">Синтетические устройства</p><h1>Доступ</h1><p>Подтверждение обязательно для каждой точки входа.</p></div></section><section className="content-card"><div className="section-title-row"><div><h2>Точки доступа</h2><p>Команды в preview не уходят в домофон.</p></div><span className="preview-pill">Демо</span></div>{accessList}</section></>

  const cameras = <><section className="page-heading"><div><p className="eyebrow">Синтетический поток</p><h1>{archive ? 'Архив камеры' : 'Камеры'}</h1><p>Камера у подъезда · данные отображаются только для preview.</p></div><div className="segmented"><button className={!archive ? 'selected' : ''} onClick={() => setArchive(false)} type="button">Прямой эфир</button><button className={archive ? 'selected' : ''} aria-label="Архив за сегодня" onClick={() => setArchive(true)} type="button">Архив за сегодня</button></div></section><section className="camera-layout"><article className="camera-card"><CameraScene archive={archive} /></article><aside className="content-card timeline"><span className="eyebrow">{archive ? 'Запись за 6 августа' : 'Статус камеры'}</span><h2>{archive ? '09:42 — событие движения' : 'Поток доступен'}</h2><p>{archive ? 'Шкала не связана с реальным архивом.' : 'Время и кадр созданы для демонстрации.'}</p><div className="timeline-line"><button type="button" aria-label="Событие в 09:42" /><i /><i /><i /></div><button className="primary-button" onClick={() => showNotice('Фрагмент подготовлен только для демонстрации')} type="button">Скачать фрагмент</button></aside></section></>

  const incomingCall = <div className="call-actions"><button className="secondary-button" onClick={() => askCommand('Игнорировать вызов?', 'Подтвердить игнорирование', () => setCall('recent'))} type="button">Игнорировать</button><button className="secondary-button danger-outline" onClick={() => askCommand('Отклонить вызов?', 'Подтвердить отклонение', () => setCall('recent'))} type="button">Отклонить</button><button className="primary-button" onClick={() => askCommand('Принять вызов?', 'Подтвердить приём вызова', () => setCall('active'))} type="button">Принять вызов</button></div>
  const activeCall = <><div className="call-controls"><button className={muted ? 'pressed' : ''} aria-label="Микрофон" onClick={() => askCommand('Изменить микрофон?', 'Подтвердить изменение микрофона', () => setMuted(!muted))} type="button"><Glyph>●</Glyph>{muted ? 'Выкл. микрофон' : 'Микрофон'}</button><button className={cameraOn ? 'pressed' : ''} aria-label="Камера" onClick={() => askCommand('Изменить камеру?', 'Подтвердить изменение камеры', () => setCameraOn(!cameraOn))} type="button"><Glyph>◉</Glyph>Камера</button><button className={soundOn ? 'pressed' : ''} aria-label="Звук" onClick={() => askCommand('Изменить звук?', 'Подтвердить изменение звука', () => setSoundOn(!soundOn))} type="button"><Glyph>♪</Glyph>Звук</button><button aria-label="Аудиовыход" onClick={() => askCommand('Изменить аудиовыход?', 'Подтвердить смену аудиовыхода', () => setSpeaker(speaker === 'browser' ? 'system' : 'browser'))} type="button"><Glyph>◌</Glyph>{speaker === 'browser' ? 'Браузер' : 'Система'}</button></div><div className="call-actions"><button className="secondary-button" onClick={() => askDoor('Подъезд', true)} type="button">Открыть дверь во время звонка</button><button className="primary-button danger" onClick={() => askCommand('Завершить звонок?', 'Подтвердить завершение звонка', () => setCall('recent'))} type="button">Завершить звонок</button></div></>
  const calls = <><section className="page-heading"><div><p className="eyebrow">Звонки · синтетические данные</p><h1>Звонки</h1><p>Входящий, активный и недавние вызовы в одном журнале.</p></div></section><section className={`call-stage ${call === 'active' ? 'active-call' : ''}`}><span className="call-avatar"><Glyph>☎</Glyph></span><span className="live-badge"><i />{call === 'incoming' ? 'ВХОДЯЩИЙ' : call === 'active' ? 'НА СВЯЗИ' : 'НЕДАВНИЙ'}</span><h2>{call === 'incoming' ? 'Подъезд · домофон' : call === 'active' ? 'Разговор с подъездом' : 'Вызов завершён'}</h2><p>{call === 'incoming' ? 'Кто-то звонит в вашу квартиру' : call === 'active' ? '00:18 · защищённый preview-канал' : 'Сегодня, 10:12 · 18 секунд'}</p>{call === 'incoming' && incomingCall}{call === 'active' && activeCall}</section><section className="content-card recent-calls"><span className="eyebrow">Недавние</span><h2>Журнал вызовов</h2><div className="event-row"><span className="event-art"><Glyph>☎</Glyph></span><span><b>Подъезд</b><small>Вчера, 20:17 · пропущен</small></span></div></section></>

  const events = <><section className="page-heading"><div><p className="eyebrow">Синтетический журнал</p><h1>События</h1><p>Только события домофона, камер и звонков.</p></div></section><section className="content-card">{['Сегодня, 09:42 · Дверь открыта', 'Сегодня, 09:38 · Движение у подъезда', 'Вчера, 20:17 · Пропущенный звонок'].map((event, index) => <button className="event-row" onClick={() => selectPage(index === 1 ? 'cameras' : index === 2 ? 'calls' : 'access')} type="button" key={event}><span className="event-art"><Glyph>{index === 1 ? '◉' : index === 2 ? '☎' : '⌑'}</Glyph></span><span><b>{event}</b><small>Демо-событие · переход доступен</small></span><Glyph>›</Glyph></button>)}</section></>

  const settings = <><section className="page-heading"><div><p className="eyebrow">Настройки звонков</p><h1>Связь и приватность</h1><p>Настройки действуют только в интерфейсе preview.</p></div></section><section className="settings-grid"><article className="content-card"><span className="eyebrow">Browser Notifications</span><h2>Уведомления браузера</h2><p>Разрешение запрашивается локально в браузере. На сервер ничего не передаётся.</p><button className="primary-button" onClick={() => askCommand('Запросить уведомления браузера?', 'Подтвердить запрос уведомлений', () => undefined, 'Запрос уведомлений доступен только в браузере')} type="button">Запросить уведомления браузера</button></article><article className="content-card"><span className="eyebrow">Блокировка</span><h2>Заблокированные номера</h2><p>Номер не получит уведомления о будущих вызовах в этом preview.</p><button className="secondary-button" onClick={() => askCommand('Заблокировать номер?', 'Подтвердить блокировку', () => undefined, 'Номер заблокирован только в предпросмотре')} type="button">Заблокировать номер +7 999 111-22-33</button></article></section></>

  const content = page === 'home' ? dashboard : page === 'access' ? access : page === 'cameras' ? cameras : page === 'calls' ? calls : page === 'events' ? events : settings

  return <div className="app-frame">
    <a className="skip-link" href="#main-content">К основному содержимому</a>
    <aside className="sidebar" aria-label="Основная навигация">
      <div className="brand"><span className="brand-mark">V</span><span>VDome <b>Preview</b></span></div>
      <nav className="nav-list">{navigation.map((item) => <button className={`nav-item ${page === item.page ? 'active' : ''}`} onClick={() => selectPage(item.page)} key={item.page} type="button"><Glyph>{item.glyph}</Glyph><span>{item.label}</span></button>)}</nav>
      <div className="sidebar-foot"><button className={`nav-item ${page === 'settings' ? 'active' : ''}`} onClick={() => selectPage('settings')} type="button"><Glyph>⚙</Glyph><span>Настройки звонков</span></button><small><span className="status-dot" />Локальный preview</small></div>
    </aside>
    <div className="app-body"><header className="topbar"><div className="flat-wrap"><button className="flat-switcher" aria-expanded={flatOpen} aria-haspopup="menu" onClick={() => setFlatOpen(!flatOpen)} type="button"><span className="flat-icon"><Glyph>⌂</Glyph></span><span><small>Текущий объект</small><b>{flat}</b></span><Glyph>⌄</Glyph></button>{flatOpen && <div className="flat-menu" role="menu"><button role="menuitem" onClick={() => { setFlat('Демо-квартира · Подъезд 1'); setFlatOpen(false) }} type="button">Демо-квартира · Подъезд 1</button><button role="menuitem" onClick={() => { setFlat('Демо-квартира · Корпус B'); setFlatOpen(false) }} type="button">Демо-квартира · Корпус B</button></div>}</div><div className="top-actions"><span className="preview-pill"><span />Предпросмотр</span><button className="icon-button" aria-label="Уведомления" onClick={() => selectPage('settings')} type="button"><Glyph>●</Glyph></button><button className="avatar" aria-label="Профиль подключения" onClick={() => setAuthOpen(true)} type="button">М</button></div></header><main id="main-content" className="main-content">{content}</main></div>
    <nav className="mobile-nav" aria-label="Мобильная навигация">{navigation.slice(0, 4).map((item) => <button className={page === item.page ? 'active' : ''} onClick={() => selectPage(item.page)} key={item.page} type="button"><Glyph>{item.glyph}</Glyph><span>{item.label}</span></button>)}</nav>
    {authOpen && <AuthPage onClose={closeAuth} onAuthenticated={() => { closeAuth(); showNotice('Демо-вход завершён: реальная авторизация не выполнялась') }} />}
    {confirmation && <Modal labelledBy="confirm-title" onClose={closeConfirmation} closeOnBackdrop>
      <button className="dialog-close" aria-label="Закрыть" onClick={closeConfirmation} type="button">×</button>
      <span className="dialog-icon"><Glyph>⌑</Glyph></span>
      <h2 id="confirm-title">{confirmation.title}</h2>
      <p>{confirmation.copy}</p>
      <div className="dialog-actions"><button className="secondary-button" data-dialog-initial onClick={closeConfirmation} type="button">Отмена</button><button className="primary-button danger" onClick={executeConfirmation} type="button">{confirmation.action}</button></div>
    </Modal>}
    {notice && <div className="toast" role="status"><span>i</span>{notice}</div>}
  </div>
}

export default App
