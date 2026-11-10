declare module "@novnc/novnc" {
  export default class RFB extends EventTarget {
    constructor(target:HTMLElement,url:string,options?:{shared?:boolean,wsProtocols?:string[]});
    viewOnly:boolean;scaleViewport:boolean;resizeSession:boolean;
    disconnect():void;
    focus(options?:FocusOptions):void;
  }
}
