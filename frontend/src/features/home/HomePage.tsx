import type { ReactNode } from 'react'

export interface HomePageProps {
  camera: ReactNode
  doors: ReactNode
  autoOpen: ReactNode | null
  activity: ReactNode
  onOpenAllEvents(): void
}

export function HomePage(props: HomePageProps) {
  return <div className="home-page">
    <h1 className="visually-hidden" tabIndex={-1}>Главная</h1>
    <section className="home-section home-camera"><h2 className="visually-hidden">Камеры</h2>{props.camera}</section>
    <div className="home-side">
      <section className="home-section home-doors"><header><h2>Двери</h2></header>{props.doors}</section>
      {props.autoOpen}
      <section className="home-section home-events">
        <header><h2>Последние события</h2><button onClick={props.onOpenAllEvents} type="button">Все события</button></header>
        {props.activity}
      </section>
    </div>
  </div>
}
