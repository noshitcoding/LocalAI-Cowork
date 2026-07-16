const fs = require('fs');

const cssPath = 'app/src/index.css';
let css = fs.readFileSync(cssPath, 'utf8');

if (!css.includes('.bg-grid-pattern')) {
    css += `
@layer utilities {
  .bg-grid-pattern {
    background-size: 24px 24px;
    background-image: linear-gradient(to right, rgba(0, 0, 0, 0.05) 1px, transparent 1px),
                      linear-gradient(to bottom, rgba(0, 0, 0, 0.05) 1px, transparent 1px);
  }
}
`;
    fs.writeFileSync(cssPath, css);
}
console.log('CSS updated');
