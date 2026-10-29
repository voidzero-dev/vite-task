import depA from 'dep-a';
import depB from 'dep-b';

export default () => ({ depA: depA(), depB: depB() });
