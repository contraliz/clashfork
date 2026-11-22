import { getWindowControls } from '../hooks'
import { Icon } from './Icon'

export function Titlebar() {
  const controls = getWindowControls()
  return (
    <>
      <div className="titlebar-drag" data-tauri-drag-region />
      <div className="window-controls">
        <button type="button" className="window-button" aria-label="Minimize" onClick={() => void controls.minimize()}>
          <Icon name="minimize" size={15} />
        </button>
        <button type="button" className="window-button" aria-label="Maximize or restore" onClick={() => void controls.maximize()}>
          <Icon name="maximize" size={13} />
        </button>
        <button type="button" className="window-button window-close" aria-label="Close to tray" onClick={() => void controls.close()}>
          <Icon name="close" size={15} />
        </button>
      </div>
    </>
  )
}
