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
    <header className="screen-heading"><h1>Главная</h1></header>
    <section className="home-section home-camera"><header><h2>Камеры</h2></header>{props.camera}</section>
    <section className="home-section home-doors"><header><h2>Двери</h2></header>{props.doors}</section>
    {props.autoOpen}
    <section className="home-section home-events">
      <header><h2>Последние события</h2><button onClick={props.onOpenAllEvents} type="button">Все события</button></header>
      {props.activity}
    </section>
  </div>
}
